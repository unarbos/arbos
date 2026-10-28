import Foundation

/// One classifier for every failure the user sees (Jacob, 2026-10-26, [feedback
/// error-messages-ios-2026-10-26]; the Mac twin is `sync/user_error.rs`). A transport / relay /
/// local error becomes a `UserError` — a **verb** (what the user tried: "generate the image"),
/// a **class** (why it failed), the **call** it was (for Details) and the **raw** words (for
/// Copy). The UI draws every one of them through `ErrorNote` and nothing else: no raw URLs, HTTP
/// verbs, ids, `localizedDescription`s or error chains ever reach a sentence.
///
/// Sentence shape: "Couldn't <verb>. <why>." — the why-clause is one of the ten classes.
struct UserError: Equatable, Hashable {
    enum UploadFault: Equatable, Hashable { case unreadable, tooLarge, unsupported }

    enum Class: Equatable, Hashable {
        case timeout
        /// `deviceOffline`: no route at all (DNS, no connection); else the relay does not answer.
        case offline(deviceOffline: Bool)
        /// A 4xx the relay explained (`message` is its sentence when it has one).
        case refused(message: String?)
        case notFound
        case server
        case rateLimited
        case contentRefused
        case upload(UploadFault)
        case authExpired
        case unknown
    }

    /// What the note offers after the sentence.
    enum Action: Equatable { case tryAgain, signIn, none }

    /// What the user tried, as an infinitive phrase: "send your message", "save the secret".
    /// Empty for a chat-level note whose words are the whole sentence.
    var verb: String
    var `class`: Class
    /// The request, when known: "POST /projects/01M5N1…ESK7X/avatar/generate".
    var call: String?
    /// The untrimmed raw words (the NSError description, the relay's body, the OS error).
    var raw: String
    /// When it happened.
    var at: Date = Date()

    // MARK: - constructors

    init(verb: String, class: Class, raw: String, call: String? = nil) {
        self.verb = verb
        self.class = `class`
        self.raw = raw
        self.call = call ?? UserError.callIn(raw)
    }

    /// Classify any `Error`: a typed `RelayError` first, then `URLError` / `NSURLErrorDomain`,
    /// then the words.
    static func classify(_ verb: String, _ error: Error) -> UserError {
        if let relay = error as? RelayError { return fromRelay(verb, relay) }
        if let user = error as? UserError { return UserError(verb: verb, class: user.class, raw: user.raw, call: user.call) }
        let ns = error as NSError
        let raw = "\(ns.domain) \(ns.code): \(ns.localizedDescription)"
        if ns.domain == NSURLErrorDomain {
            switch ns.code {
            case NSURLErrorTimedOut:
                return UserError(verb: verb, class: .timeout, raw: raw)
            case NSURLErrorNotConnectedToInternet, NSURLErrorDataNotAllowed, NSURLErrorInternationalRoamingOff,
                 NSURLErrorCannotFindHost, NSURLErrorDNSLookupFailed:
                return UserError(verb: verb, class: .offline(deviceOffline: true), raw: raw)
            case NSURLErrorCannotConnectToHost, NSURLErrorNetworkConnectionLost, NSURLErrorSecureConnectionFailed,
                 NSURLErrorServerCertificateUntrusted, NSURLErrorBadServerResponse, NSURLErrorCannotLoadFromNetwork:
                return UserError(verb: verb, class: .offline(deviceOffline: false), raw: raw)
            case NSURLErrorUserAuthenticationRequired:
                return UserError(verb: verb, class: .authExpired, raw: raw)
            case NSURLErrorFileDoesNotExist, NSURLErrorResourceUnavailable:
                return UserError(verb: verb, class: .notFound, raw: raw)
            case NSURLErrorCannotDecodeRawData, NSURLErrorCannotDecodeContentData, NSURLErrorDataLengthExceedsMaximum:
                return UserError(verb: verb, class: .upload(.unreadable), raw: raw)
            default:
                break
            }
        }
        if ns.domain == NSCocoaErrorDomain {
            switch ns.code {
            case NSFileNoSuchFileError, NSFileReadNoSuchFileError: return UserError(verb: verb, class: .notFound, raw: raw)
            case NSFileReadNoPermissionError, NSFileReadCorruptFileError, NSFileReadUnknownError:
                return UserError(verb: verb, class: .upload(.unreadable), raw: raw)
            default: break
            }
        }
        return UserError(verb: verb, class: classForWords(raw), raw: raw)
    }

    /// Classify a typed relay answer.
    static func fromRelay(_ verb: String, _ e: RelayError) -> UserError {
        let cls = classForStatus(e.status, kind: e.type ?? "", sentence: relaySentence(e))
        var raw = "HTTP \(e.status)"
        if let t = e.type, !t.isEmpty { raw += " \(t)" }
        if !e.message.isEmpty { raw += " — \(e.message)" }
        if let d = e.detail, !d.isEmpty { raw += " — \(d)" }
        return UserError(verb: verb, class: cls, raw: raw)
    }

    /// Classify an HTTP status alone.
    static func fromStatus(_ verb: String, _ status: Int, body: String = "") -> UserError {
        fromRelay(verb, RelayError(status: status, message: body))
    }

    /// Classify a failure that only exists as text.
    static func fromText(_ verb: String, _ raw: String) -> UserError {
        UserError(verb: verb, class: classForWords(raw), raw: raw)
    }

    /// A failure the code itself diagnosed — the class is known, `note` is for Details / Copy.
    static func of(_ verb: String, _ cls: Class, _ note: String = "") -> UserError {
        UserError(verb: verb, class: cls, raw: note)
    }

    /// A local refusal (the form is not complete): the words as the why.
    static func refused(_ verb: String, _ why: String) -> UserError {
        UserError(verb: verb, class: .refused(message: why), raw: "")
    }

    // MARK: - words

    /// The one sentence the user reads.
    var sentence: String {
        let why = self.why
        if verb.isEmpty { return why }
        return why.isEmpty ? "Couldn't \(verb)." : "Couldn't \(verb). \(why)"
    }

    /// The why-clause alone.
    var why: String {
        switch `class` {
        case .timeout: return "The server took too long."
        case .offline(deviceOffline: true): return "You're offline."
        case .offline(deviceOffline: false): return "Arbos can't reach the server right now."
        case .refused(message: let m?): return UserError.ensurePeriod(m)
        case .refused(message: nil): return "That wasn't accepted."
        case .notFound: return "It isn't there any more."
        case .server: return "The server had a problem."
        case .rateLimited: return "Too many requests — wait a moment."
        case .contentRefused: return "That prompt was refused — try different words."
        case .upload(.unreadable): return "The file couldn't be read."
        case .upload(.tooLarge): return "That file is too large (25 MB max)."
        case .upload(.unsupported): return "That file type isn't supported."
        case .authExpired: return "You were signed out."
        case .unknown: return "Something went wrong."
        }
    }

    /// Try again where a retry can help; Sign in on 401 / 403; nothing where the input must
    /// change (content refused, 404, size / type, a 4xx with the relay's reason).
    var action: Action {
        switch `class` {
        case .timeout, .offline, .server, .rateLimited, .unknown, .upload(.unreadable): return .tryAgain
        case .authExpired: return .signIn
        case .refused, .notFound, .contentRefused, .upload(.tooLarge), .upload(.unsupported): return .none
        }
    }

    /// The Details block: what · call · where / when / build — never the whole chain.
    var details: [String] {
        var out = [whatLine]
        if let call { out.append(UserError.shortenIds(call)) }
        let host = UserError.hostIn(raw) ?? "this iPhone"
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "dev"
        out.append("\(host) · \(UserError.stamp(at)) · iPhone \(version)")
        return out
    }

    /// What goes on the pasteboard for a feedback report.
    var copyText: String {
        var text = sentence + "\n" + details.joined(separator: "\n")
        if !raw.isEmpty { text += "\n\n" + raw }
        return text
    }

    private var whatLine: String {
        let status = UserError.statusIn(raw)
        switch (`class`, status) {
        case (.timeout, _):
            if let s = UserError.timeoutSeconds(raw) { return "Timed out after \(s) s" }
            return "Timed out"
        case (.offline(let device), _):
            return device ? "No network route" : "Connection refused or reset"
        case (_, let (code, reason)?) where !reason.isEmpty:
            return "HTTP \(code) \(reason) from the relay"
        case (_, let (code, _)?):
            return "HTTP \(code) from the relay"
        case (.upload, nil):
            return "The file could not be read or encoded"
        default:
            return UserError.firstLine(raw) ?? "Unexpected failure"
        }
    }

    // MARK: - classification helpers

    private static func relaySentence(_ e: RelayError) -> String? {
        let m = e.message.trimmingCharacters(in: .whitespacesAndNewlines)
        if m.isEmpty || m.hasPrefix("HTTP ") || m.count > 160 || m.contains("://") { return nil }
        return m
    }

    static func classForStatus(_ status: Int, kind: String, sentence: String?) -> Class {
        if (kind.contains("refused") && (kind.contains("prompt") || kind.contains("content")))
            || kind == "moderation" || kind == "content_policy"
            || (kind.hasSuffix("_refused") && (sentence ?? "").lowercased().contains("prompt")) {
            // The relay's `avatar_refused` carries "That prompt was refused — …": the same class.
            return .contentRefused
        }
        if kind.contains("timeout") { return .timeout }
        if kind == "too_large" || kind == "payload_too_large" { return .upload(.tooLarge) }
        if kind == "unsupported_media" || kind == "unsupported_type" { return .upload(.unsupported) }
        if kind == "rate_limited" { return .rateLimited }
        // A 403 that is a permission answer, not a dead session: the relay names the rule
        // (`admin_only`, `owner_only`, `not_member`, …) and says it in one sentence. The bare
        // `forbidden` / `token_expired` kinds are the session.
        if status == 403, !kind.isEmpty, !["forbidden", "token_expired", "unauthorized"].contains(kind) {
            return .refused(message: sentence)
        }
        switch status {
        case 401, 403: return .authExpired
        case 404, 410: return .notFound
        case 408, 504: return .timeout
        case 413: return .upload(.tooLarge)
        case 415: return .upload(.unsupported)
        case 429: return .rateLimited
        case 400...499: return .refused(message: sentence)
        case 500...599: return .server
        default: return .unknown
        }
    }

    private static func rawSaysOffline(_ raw: String) -> Bool {
        let d = raw.lowercased()
        return ["dns error", "failed to lookup address", "not connected to the internet", "offline",
                "name or service not known", "nodename nor servname", "network is unreachable", "host is down"]
            .contains { d.contains($0) }
    }

    /// The words alone (no typed cause).
    static func classForWords(_ raw: String) -> Class {
        let d = raw.lowercased()
        if let hit = statusIn(raw) { return classForStatus(hit.0, kind: "", sentence: nil) }
        if d.contains("timed out") || d.contains("timeout") || d.contains("took too long") || d.contains("did not answer in time") {
            return .timeout
        }
        if rawSaysOffline(raw) { return .offline(deviceOffline: true) }
        if ["could not connect", "connection refused", "connection reset", "connection lost", "tls", "can't reach",
            "cannot reach", "could not reach", "not reachable", "connection appears to be offline"].contains(where: { d.contains($0) }) {
            return .offline(deviceOffline: false)
        }
        if d.contains("rate limit") || d.contains("too many requests") { return .rateLimited }
        if d.contains("prompt was refused") || d.contains("content policy") || d.contains("moderation") { return .contentRefused }
        if d.contains("too large") { return .upload(.tooLarge) }
        if d.contains("unsupported") && (d.contains("type") || d.contains("format")) { return .upload(.unsupported) }
        if d.contains("signed out") || d.contains("unauthorized") || d.contains("token expired") || d.contains("session ended") {
            return .authExpired
        }
        if d.contains("not found") || d.contains("no such file") { return .notFound }
        if d.contains("bad gateway") || d.contains("gateway timeout") || d.contains("server error") { return .server }
        return .unknown
    }

    /// `HTTP 504 avatar_timeout …` → (504, "avatar_timeout"); `HTTP 404` → (404, "").
    static func statusIn(_ raw: String) -> (Int, String)? {
        guard let range = raw.range(of: "HTTP ") else { return nil }
        let rest = raw[range.upperBound...]
        let digits = rest.prefix { $0.isNumber }
        guard let code = Int(digits), (100...599).contains(code) else { return nil }
        let after = rest.dropFirst(digits.count).drop { $0 == " " }
        let token = after.prefix { !" —:\n".contains($0) }
        let reason = token.allSatisfy { $0.isLowercase || $0.isNumber || $0 == "_" } && !token.isEmpty ? String(token) : ""
        return (code, reason)
    }

    /// "… operation timed out after 40 s …" → 40.
    static func timeoutSeconds(_ raw: String) -> Int? {
        let d = raw.lowercased()
        guard let range = d.range(of: "after ") else { return nil }
        let digits = d[range.upperBound...].prefix { $0.isNumber }
        return Int(digits)
    }

    /// "POST /projects/x/avatar/generate" out of the raw words.
    static func callIn(_ raw: String) -> String? {
        for verb in ["POST ", "GET ", "PUT ", "DELETE ", "PATCH "] {
            guard let range = raw.range(of: verb) else { continue }
            let rest = raw[range.lowerBound...]
            let line = rest.prefix { !":\n)".contains($0) }.trimmingCharacters(in: .whitespaces)
            if line.count > verb.count { return line }
        }
        return nil
    }

    /// "https://relay.arbos.life/…" → "relay.arbos.life".
    static func hostIn(_ raw: String) -> String? {
        guard let range = raw.range(of: "://") else { return nil }
        let host = raw[range.upperBound...].prefix { !"/) :\n".contains($0) }
        return host.isEmpty ? nil : String(host)
    }

    /// ULIDs and long tokens shortened in the middle: `01M5N1WSR91G8CJKNYXKJESK7X` → `01M5N1…ESK7X`.
    static func shortenIds(_ s: String) -> String {
        s.split(separator: "/", omittingEmptySubsequences: false).map { seg -> String in
            let alnum = seg.allSatisfy { $0.isLetter || $0.isNumber || $0 == "_" || $0 == "-" }
            if alnum, seg.count >= 20, seg.contains(where: \.isNumber) {
                return "\(seg.prefix(6))…\(seg.suffix(5))"
            }
            return String(seg)
        }.joined(separator: "/")
    }

    private static func firstLine(_ raw: String) -> String? {
        guard let line = raw.split(separator: "\n").first?.trimmingCharacters(in: .whitespaces), !line.isEmpty else { return nil }
        return line.count > 120 ? String(line.prefix(120)) + "…" : line
    }

    private static func ensurePeriod(_ s: String) -> String {
        let t = s.trimmingCharacters(in: .whitespaces)
        return t.hasSuffix(".") || t.hasSuffix("!") || t.hasSuffix("?") ? t : t + "."
    }

    private static let stampFormatter: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "UTC")
        f.dateFormat = "HH:mm:ss 'UTC'"
        return f
    }()

    static func stamp(_ date: Date) -> String { stampFormatter.string(from: date) }

    // MARK: - Equatable / Hashable: the words, not the clock

    static func == (a: UserError, b: UserError) -> Bool {
        a.verb == b.verb && a.class == b.class && a.call == b.call && a.raw == b.raw
    }

    func hash(into hasher: inout Hasher) {
        hasher.combine(verb)
        hasher.combine(raw)
    }
}

extension UserError: Error {}
