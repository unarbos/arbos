import Foundation

/// Everything the user can change. Plain values live in UserDefaults;
/// the OpenAI key lives in the Keychain and is only mirrored here in memory.
///
/// Orb product: server URL + OpenAI API key. Nothing else.
@MainActor
final class AppSettings: ObservableObject {
    private static let openAIKeyAccount = "openai-api-key"
    private static let voiceTokenAccount = "voice-server-token"
    private static let kernelTokenAccount = "kernel-token"
    private static let hubTokenAccount = "hub-token"

    static let callInstructions = """
    You are a brief voice companion. Answer greetings in one short sentence. \
    Anything about code, files, projects, or work: say you will check and wait \
    for the result. Never invent results.
    """

    @Published var provider: VoiceProvider = .selfHosted {
        didSet { defaults.set(provider.rawValue, forKey: "voiceProvider") }
    }
    @Published var openAIModel: String = "gpt-live-1" {
        didSet { defaults.set(openAIModel, forKey: "openAIModel") }
    }
    /// `wss://host/ws` — the voice server that runs GPT-Live + Codex.
    @Published var selfHostedURL: String = "" {
        didSet { defaults.set(selfHostedURL, forKey: "selfHostedURL") }
    }
    @Published var kernelURL: String = "" {
        didSet { defaults.set(kernelURL, forKey: "kernelURL") }
    }
    @Published var hubURL: String = "" {
        didSet { defaults.set(hubURL, forKey: "hubURL") }
    }
    @Published var frontProject: String? {
        didSet { defaults.set(frontProject, forKey: "frontProject") }
    }
    @Published var kernelTarget: KernelTarget = .pod {
        didSet { defaults.set(kernelTarget.stored, forKey: "kernelTarget") }
    }
    @Published private(set) var openAIKey: String = ""
    @Published private(set) var voiceToken: String = ""
    @Published private(set) var kernelToken: String = ""
    @Published private(set) var hubToken: String = ""

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let baked = BuiltInSecrets.current

        provider = .selfHosted
        openAIModel = defaults.string(forKey: "openAIModel") ?? "gpt-live-1"

        var url = defaults.string(forKey: "selfHostedURL") ?? ""
        if url.isEmpty { url = baked.voiceServerURL }
        if !baked.voiceServerURL.isEmpty, !ProcessInfo.processInfo.arguments.contains("-selfHostedURL") {
            let previous = defaults.string(forKey: "baked-voiceServerURL")
            if previous == nil || url == previous || url.isEmpty {
                url = baked.voiceServerURL
            }
            defaults.set(baked.voiceServerURL, forKey: "baked-voiceServerURL")
        }
        selfHostedURL = url
        defaults.set(url, forKey: "selfHostedURL")

        kernelURL = defaults.string(forKey: "kernelURL") ?? baked.kernelURL
        hubURL = defaults.string(forKey: "hubURL") ?? baked.hubURL
        kernelTarget = KernelTarget(stored: defaults.string(forKey: "kernelTarget") ?? "pod")
        frontProject = defaults.string(forKey: "frontProject")
        openAIKey = Keychain.read(Self.openAIKeyAccount) ?? ""
        #if DEBUG
        // `-previewKey sk-…` stands in for the Keychain, which a simulator built
        // fresh by CI has no way to seed. Without it the only screen that can be
        // photographed is the unconfigured one, and the states the app spends its
        // life in never get looked at. Debug builds only, and nothing is written:
        // `reloadSecrets` fills the key only when it is empty, so this survives.
        if let preview = defaults.string(forKey: "previewKey"), !preview.isEmpty {
            openAIKey = preview
        }
        #endif
        voiceToken = Keychain.read(Self.voiceTokenAccount) ?? baked.voiceToken
        kernelToken = Keychain.read(Self.kernelTokenAccount) ?? baked.kernelToken
        hubToken = Keychain.read(Self.hubTokenAccount) ?? baked.hubToken
    }

    var hubConfigured: Bool {
        !hubToken.isEmpty && URL(string: hubURL)?.host != nil
    }

    var chatEndpoint: ArbosKernelClient.Endpoint? {
        switch kernelTarget {
        case .pod:
            return kernelEndpoint
        case .hub(let machine, let project):
            guard hubConfigured, let url = HubClient.attachURL(hubURL: hubURL, machine: machine, project: project) else {
                return nil
            }
            return .init(url: url, token: hubToken)
        }
    }

    /// Server URL and OpenAI key — the only things Settings asks for.
    var isConfigured: Bool {
        serverURLProblem == nil && !openAIKey.isEmpty
    }

    var unconfiguredLabel: String {
        if selfHostedURL.isEmpty { return "set the server in Settings" }
        if serverURLProblem != nil { return "check the server address in Settings" }
        if openAIKey.isEmpty { return "set your OpenAI key in Settings" }
        return ""
    }

    /// Why the server address cannot be called, or nil when it can. A URL that
    /// is wrong in a way the app can see is worth saying before the call fails
    /// with a socket error nobody can read.
    var serverURLProblem: String? {
        Self.problem(withServerURL: selfHostedURL)
    }

    static func problem(withServerURL raw: String) -> String? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return "Enter the address of your voice server." }
        guard let url = URL(string: trimmed), let scheme = url.scheme?.lowercased() else {
            return "That is not a valid address."
        }
        guard ["ws", "wss"].contains(scheme) else {
            return "Use ws:// or wss://, not \(scheme)://."
        }
        guard let host = url.host, !host.isEmpty else { return "The address has no host." }
        return nil
    }

    /// The obvious paste mistakes forgiven: surrounding whitespace, the
    /// `https` scheme the tunnel hands out, a bare host with no scheme at all.
    /// A tunnel URL copied from a browser is the normal way this gets typed.
    static func normaliseServerURL(_ raw: String) -> String {
        var text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return "" }
        if text.lowercased().hasPrefix("https://") {
            text = "wss://" + text.dropFirst("https://".count)
        } else if text.lowercased().hasPrefix("http://") {
            text = "ws://" + text.dropFirst("http://".count)
        } else if !text.contains("://") {
            text = "wss://" + text
        }
        while text.hasSuffix("/") { text.removeLast() }
        // The voice server listens on `/ws`; a host on its own is what people
        // paste, and failing that paste teaches them nothing.
        if let url = URL(string: text), url.path.isEmpty {
            text += "/ws"
        }
        return text
    }

    var kernelEndpoint: ArbosKernelClient.Endpoint? {
        guard !kernelToken.isEmpty, let url = URL(string: kernelURL), let scheme = url.scheme,
              ["ws", "wss"].contains(scheme.lowercased()) else { return nil }
        return .init(url: url, token: kernelToken)
    }

    /// Returns false when the Keychain took the key but cannot give it back, so
    /// Settings can say so instead of looking saved and being empty next launch.
    @discardableResult
    func saveOpenAIKey(_ key: String) -> Bool {
        let stored = Keychain.write(key, account: Self.openAIKeyAccount)
        openAIKey = Keychain.read(Self.openAIKeyAccount) ?? ""
        return stored
    }

    /// Read the secrets again.
    ///
    /// A process launched before the device's first unlock gets nil from every
    /// Keychain read, and the key then looks unset for as long as that process
    /// lives — the app says "set your OpenAI key" about a key that is already
    /// saved. Called when the app comes to the front, which is the first moment
    /// the read is certain to work.
    func reloadSecrets() {
        if openAIKey.isEmpty, let key = Keychain.read(Self.openAIKeyAccount), !key.isEmpty {
            openAIKey = key
        }
        if voiceToken.isEmpty, let token = Keychain.read(Self.voiceTokenAccount), !token.isEmpty {
            voiceToken = token
        }
    }

    func saveVoiceToken(_ token: String) {
        Keychain.write(token, account: Self.voiceTokenAccount)
        voiceToken = Keychain.read(Self.voiceTokenAccount) ?? ""
    }

    func saveKernelToken(_ token: String) {
        Keychain.write(token, account: Self.kernelTokenAccount)
        kernelToken = Keychain.read(Self.kernelTokenAccount) ?? ""
    }

    func saveHubToken(_ token: String) {
        Keychain.write(token, account: Self.hubTokenAccount)
        hubToken = Keychain.read(Self.hubTokenAccount) ?? ""
    }
}
