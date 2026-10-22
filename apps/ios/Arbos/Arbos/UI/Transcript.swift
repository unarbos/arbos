// The message stream, Grok Bot style: assistant bubbles (#202020) left, user
// bubbles (#5C5C5C) right, centred muted system lines between them, file and
// voice-memo cards, one "<Project> is working" line while a turn runs.
// Prose only: tool calls, status noise, usage and JSON never render.

import ArbosFFI
import AVFoundation
import QuickLook
import SwiftUI
import UIKit

enum Block: Equatable, Hashable {
    case paragraph(String)
    case heading(level: Int, text: String)
    case bullets([String])
    case numbered([String])
    case code(lang: String, code: String)
    case table([String])
}

/// Grok Bot's quote header inside a reply bubble: speech-bubble glyph, then the replied-to
/// message's first words in quotes, one line, truncated with "…". Tapping it reveals the original.
struct Quote: Equatable, Hashable {
    /// The message replied to (`replyTo` on the row).
    var messageId: Id
    var text: String

    /// The header shows this many characters of the original before "…".
    static let maxChars = 64

    /// The quote for a message that replies to `replyTo`; an original that is not loaded
    /// (older than the snapshot) still gets a header so the reference is never lost.
    static func forReply(_ replyTo: String, in messages: [Message]) -> Quote {
        let text = messages.first { $0.id == replyTo }.map(Self.text) ?? "Earlier message"
        return Quote(messageId: replyTo, text: text)
    }

    /// First words of a message: its first non-empty line, whitespace collapsed, cut on a word
    /// boundary; an attachment-only message names its file.
    static func text(of message: Message) -> String {
        var line = message.plainText
            .split(separator: "\n")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .first { !$0.isEmpty }
            .map { $0.split(whereSeparator: \.isWhitespace).joined(separator: " ") } ?? ""
        if line.isEmpty {
            for part in message.parts {
                if case .attachment(_, let a) = part { line = a.name; break }
            }
        }
        if line.isEmpty { return "Message" }
        guard line.count > maxChars else { return line }
        var cut = String(line.prefix(maxChars))
        if let space = cut.lastIndex(of: " "), cut.distance(from: cut.startIndex, to: space) > maxChars / 2 {
            cut = String(cut[..<space])
        }
        while let last = cut.last, ",;:.".contains(last) { cut.removeLast() }
        return cut + "…"
    }
}

enum RowKind: Equatable {
    case user(text: String, pending: Bool, quote: Quote?)
    /// Another person's message in a people chat: left bubble with avatar; the name above
    /// only when the sender changed (`named`, Grok Bot's group style).
    case other(text: String, handle: String, avatar: String?, named: Bool, quote: Quote?)
    case assistant(blocks: [Block], live: Bool, caption: String?)
    /// Shared agent in a people chat: same shape as `.other` (avatar + bubble, name on sender
    /// change). No "answering @x · owned by @y" meta — ownership lives in the profile pane
    /// (Jacob, 2026-10-22). `agentId` opens that pane from the avatar / name.
    case member(blocks: [Block], live: Bool, handle: String, avatar: String?, named: Bool, agentId: String?)
    /// Centred muted line: hand-offs ("Messaged ◉ Affine"), stops, dates.
    case system(icon: String?, text: String, avatarProject: Id?)
    /// Grok Bot question card: bold question, × top-right, lettered answer rows.
    /// `questionId` is nil for a legacy `waiting_user` status line (no `question` part).
    case question(questionId: String?, text: String, options: [ChoiceOption], multi: Bool, picked: [String], open: Bool)
    case file(name: String, subtitle: String, projectId: Id, ref: String?)
    case voiceMemo(name: String, projectId: Id, ref: String?, fromUser: Bool)
    case image(projectId: Id, ref: String?, fromUser: Bool)
    case error(code: String, message: String)
}

struct Row: Identifiable, Equatable {
    var id: String
    var messageId: Id
    var role: Role
    var kind: RowKind
    /// Emoji reactions shown under this bubble (stamped on the last prose/media row of a message).
    var reactions: [MessageReaction] = []
}

enum Transcript {
    /// An agent (or person) who joined a people chat — rendered as a centred "@handle joined" line.
    struct Join: Equatable {
        var id: String
        var handle: String
        var at: Int64
    }

    /// `me` = my `user:<id>` in a people chat; nil in project chats (every user row is mine).
    /// `joins` = people-chat membership events (people/agents added mid-chat) merged by time.
    /// `agentNames`: agent project id → display name from the chat's member list, the
    /// fallback when a reply row carries no `senderName` (older boxes).
    static func rows(_ messages: [Message], projectId: Id, subagentNames: [String: String] = [:], me: String? = nil, people: Bool = false, joins: [Join] = [], agentNames: [String: String] = [:]) -> [Row] {
        var rows: [Row] = []
        var lastAt: Int64 = 0
        // Who spoke last: a name sits above a bubble only when the sender changed or a date
        // line broke the run (Grok Bot's group style).
        var prevSender: String? = nil
        var pendingJoins = joins.filter { $0.at > 0 && !$0.handle.isEmpty }.sorted { $0.at < $1.at }

        /// Emit join lines whose time is at or before `before` (or all remaining when `before` is nil).
        func flushJoins(before: Int64?) {
            while let j = pendingJoins.first {
                if let before, j.at > before { break }
                pendingJoins.removeFirst()
                if j.at > 0 && (lastAt == 0 || j.at - lastAt > 3_600_000) {
                    rows.append(Row(id: "join-\(j.id)#date", messageId: "join-\(j.id)", role: .system,
                                    kind: .system(icon: nil, text: dateLabel(j.at), avatarProject: nil)))
                }
                rows.append(Row(id: "join-\(j.id)", messageId: "join-\(j.id)", role: .system,
                                kind: .system(icon: nil, text: "@\(j.handle) joined", avatarProject: nil)))
                if j.at > 0 { lastAt = j.at }
            }
        }

        /// Hand-off rows by id (name, messages exchanged), merged into one line when consecutive.
        var handoffs: [String: (String, Int)] = [:]
        for message in messages {
            // Voice-call transcript lines stay in the doc for the agent; they are not
            // chat bubbles (Grok: typed prose, attachments, and call start/end notes only).
            if message.voice { continue }
            // Build this message's visible rows first. Date separators only land when
            // something actually renders — otherwise an empty/hidden-only message leaves
            // a lone "Today 5:45 PM" in a blank chat.
            var chunk: [Row] = []
            let streaming = message.isStreaming
            let dateBreak = message.createdAt > 0 && (lastAt == 0 || message.createdAt - lastAt > 3_600_000)
            let named = dateBreak || prevSender != message.sender
            switch message.role {
            case .user:
                var hasImage = false
                for part in message.parts {
                    if case .attachment(let id, let a) = part {
                        if a.isImage { hasImage = true }
                        chunk.append(attachmentRow(id: "\(message.id)#\(id)", message: message, a: a, projectId: projectId, fromUser: true))
                    }
                }
                let text = message.plainText
                // Generic labels ("Image", "Photo", …) are not prose — the attachment
                // row already carries the media. Showing them as a bubble is noise.
                if !text.isEmpty, !(hasImage && Self.isGenericMediaLabel(text)) {
                    let mine = !people || me == nil || message.sender == me || message.sender == "user"
                    let quote = message.replyTo.flatMap { $0.isEmpty ? nil : Quote.forReply($0, in: messages) }
                    if mine {
                        chunk.append(Row(id: "\(message.id)#user", messageId: message.id, role: .user,
                                         kind: .user(text: text, pending: message.pending, quote: quote)))
                    } else {
                        chunk.append(Row(id: "\(message.id)#other", messageId: message.id, role: .project,
                                         kind: .other(text: text, handle: message.senderHandle ?? "someone", avatar: message.senderAvatar, named: named, quote: quote)))
                    }
                }
            case .system:
                // Call lines and other system notes: one muted centred line.
                let text = message.plainText.trimmingCharacters(in: .whitespacesAndNewlines)
                if !text.isEmpty {
                    let icon = text.hasPrefix("Call ended") ? "phone.down.fill" : (text.contains(" call") ? "phone.fill" : nil)
                    chunk.append(Row(id: "\(message.id)#system", messageId: message.id, role: .system,
                                     kind: .system(icon: icon, text: text, avatarProject: nil)))
                }
            case .assistant, .project:
                // Hand-off: Grok shows "Messaged ◉ <bot>" / "N messages with ◉ A, ◉ B and ◉ C"
                // as a system line. Consecutive hand-offs merge below.
                let handoffName = (message.turn == .new || message.turn?.isProgress == true)
                    ? (message.subagentName ?? subagentNames[message.subagentId ?? ""]) : nil
                if let name = handoffName {
                    let rowId = "\(message.id)#handoff"
                    handoffs[rowId] = (name, max(1, message.handoffMessages ?? 1))
                    chunk.append(Row(id: rowId, messageId: message.id, role: .system,
                                     kind: .system(icon: nil, text: Self.handoffText([(name, max(1, message.handoffMessages ?? 1))]), avatarProject: projectId)))
                }
                let lastTextPart = message.parts.lastIndex { $0.textValue != nil }
                let firstTextPart = message.parts.firstIndex { $0.textValue != nil }
                let agentId: String? = message.sender.hasPrefix("agent:") ? String(message.sender.dropFirst(6)) : nil
                // Error parts wait until the prose is known: under a real answer they stay the
                // chip with Retry; on an answer with no words they become the words.
                var errors: [(id: Id, code: String, text: String)] = []
                let rowsBeforeParts = chunk.count
                for (pix, part) in message.parts.enumerated() {
                    switch part {
                    case .text(let id, let text):
                        // Polish on display (Mac parity): glued "findingsDon't" / "have.Checking"
                        // and a trailing tool-narration sentence still read as chat.
                        let raw = people ? Self.stripAskedPrefix(text) : text
                        let body = Markdown.polishAssistantProse(raw)
                        let blocks = Markdown.blocks(body).filter { !Markdown.isJSON($0) }
                        if blocks.isEmpty { continue }
                        let live = streaming && pix == lastTextPart
                        if people, let h = message.senderHandle {
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                             kind: .member(blocks: blocks, live: live, handle: h,
                                                           avatar: message.senderAvatar, named: named && pix == firstTextPart,
                                                           agentId: agentId)))
                        } else {
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                             kind: .assistant(blocks: blocks, live: live, caption: nil)))
                        }
                    case .status(let id, .askUser, let text):
                        // agentd writes a `question` part and this status line; the part wins.
                        if message.questions.isEmpty {
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                             kind: .question(questionId: nil, text: text, options: [], multi: false, picked: [], open: true)))
                        }
                    case .question(let id, let q):
                        chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                         kind: .question(questionId: q.questionId, text: q.question, options: q.options,
                                                         multi: q.multi, picked: q.answer, open: q.open)))
                    case .status(let id, .handoff, let text):
                        // Legacy status line; the turn-based row above already covers it.
                        if handoffName == nil {
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .system,
                                             kind: .system(icon: nil, text: handoffText(text), avatarProject: nil)))
                        }
                    case .status(let id, .stopped, _):
                        chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .system,
                                         kind: .system(icon: "stop.circle", text: "Stopped", avatarProject: nil)))
                    case .attachment(let id, let a):
                        chunk.append(attachmentRow(id: "\(message.id)#\(id)", message: message, a: a, projectId: projectId, fromUser: false))
                    case .error(let id, let code, let text):
                        errors.append((id: id, code: code, text: text))
                    case .status, .tool, .subagentResult, .reasoning, .unknown:
                        break
                    }
                }
                // A finished reply that shows nothing — no prose, no card, no attachment — used
                // to vanish here (Mac: a bare "answering @…" header; Lium in the "Testing"
                // group, 2026-10-21). Say so in the bubble, plainly, like the limit
                // announcements: the error part's own line when the box wrote one, else a
                // stock line. A row that only did work (tool calls) may end without words —
                // its answer came as a progress bubble — so only an all-empty row gets the
                // stock line.
                let showsNothing = chunk.count == rowsBeforeParts
                let finished = message.status == .done || message.status == .error
                let onlyEmpty = message.parts.allSatisfy { part in
                    switch part {
                    case .text, .status, .error: return true
                    case .tool, .attachment, .question, .subagentResult, .reasoning, .unknown: return false
                    }
                }
                let failed = !errors.isEmpty || message.status == .error
                if showsNothing, finished, !streaming, failed || onlyEmpty {
                    let line: String = {
                        if let first = errors.first?.text.trimmingCharacters(in: .whitespacesAndNewlines), !first.isEmpty {
                            return first
                        }
                        let fromRow = (message.senderName ?? "").trimmingCharacters(in: .whitespaces)
                        let agentId = message.sender.hasPrefix("agent:") ? String(message.sender.dropFirst(6)) : message.sender
                        let fromMembers = (agentNames[agentId] ?? "").trimmingCharacters(in: .whitespaces)
                        let name = !fromRow.isEmpty ? fromRow : fromMembers
                        return "\(name.isEmpty ? "The agent" : name) didn't answer. Ask again."
                    }()
                    let blocks: [Block] = [.paragraph(line)]
                    if people, let h = message.senderHandle {
                        chunk.append(Row(id: "\(message.id)#failed", messageId: message.id, role: .assistant,
                                         kind: .member(blocks: blocks, live: false, handle: h,
                                                       avatar: message.senderAvatar, named: named, agentId: agentId)))
                    } else {
                        chunk.append(Row(id: "\(message.id)#failed", messageId: message.id, role: .assistant,
                                         kind: .assistant(blocks: blocks, live: false, caption: nil)))
                    }
                } else {
                    for e in errors {
                        chunk.append(Row(id: "\(message.id)#\(e.id)", messageId: message.id, role: .assistant, kind: .error(code: e.code, message: e.text)))
                    }
                }
            }
            guard !chunk.isEmpty else { continue }
            if !message.reactions.isEmpty,
               let ix = chunk.lastIndex(where: {
                   switch $0.kind {
                   case .user, .other, .member, .assistant, .image, .file, .voiceMemo: return true
                   default: return false
                   }
               }) {
                chunk[ix].reactions = message.reactions
            }
            // Joins that happened before this message land first (Grok: "@bob joined").
            if message.createdAt > 0 { flushJoins(before: message.createdAt) }
            // Date separators when the gap is over an hour (Grok: "Today 8:00 AM").
            if dateBreak {
                rows.append(Row(id: "\(message.id)#date", messageId: message.id, role: .system,
                                kind: .system(icon: nil, text: dateLabel(message.createdAt), avatarProject: nil)))
            }
            if message.createdAt > 0 { lastAt = message.createdAt }
            rows.append(contentsOf: chunk)
            prevSender = message.sender
        }
        // Agent added after the latest message (the usual "I just added Bob" case).
        flushJoins(before: nil)
        return Self.mergeHandoffs(rows, handoffs)
    }

    /// Consecutive hand-off lines become one: "6 messages with ◉ Kenny, ◉ Tyler and ◉ Jenny".
    private static func mergeHandoffs(_ rows: [Row], _ handoffs: [String: (String, Int)]) -> [Row] {
        guard handoffs.count > 1 else { return rows }
        var out: [Row] = []
        var run: [(String, Int)] = []
        var runRow: Row?
        func flush() {
            guard let first = runRow else { return }
            if run.count > 1, case .system(_, _, let project) = first.kind {
                out.append(Row(id: first.id, messageId: first.messageId, role: .system,
                               kind: .system(icon: nil, text: handoffText(run), avatarProject: project)))
            } else {
                out.append(first)
            }
            run = []; runRow = nil
        }
        for row in rows {
            if let h = handoffs[row.id] {
                if runRow == nil { runRow = row }
                run.append(h)
            } else {
                flush()
                out.append(row)
            }
        }
        flush()
        return out
    }

    /// Grok's wording: "Messaged ◉ A" (brief only), "Messaged ◉ A and ◉ B", or
    /// "N messages with ◉ A, ◉ B and ◉ C" once replies came back. Each ◉ is drawn
    /// from the marker + name by `systemText`.
    static func handoffText(_ items: [(String, Int)]) -> String {
        var names: [String] = []
        var total = 0
        for (name, count) in items {
            if !names.contains(name) { names.append(name) }
            total += max(1, count)
        }
        let marked = names.map { "\u{FFFC}\($0)" }
        let joined: String
        switch marked.count {
        case 0: return ""
        case 1: joined = marked[0]
        case 2: joined = "\(marked[0]) and \(marked[1])"
        default: joined = marked.dropLast().joined(separator: ", ") + " and " + marked.last!
        }
        if total <= names.count { return "Messaged \(joined)" }
        return "\(total) messages with \(joined)"
    }

    private static func attachmentRow(id: String, message: Message, a: Attachment, projectId: Id, fromUser: Bool) -> Row {
        if a.mime.lowercased().hasPrefix("audio/") || a.kind?.lowercased() == "audio" {
            return Row(id: id, messageId: message.id, role: fromUser ? .user : .assistant,
                       kind: .voiceMemo(name: a.name, projectId: projectId, ref: a.blobRef, fromUser: fromUser))
        }
        if a.isImage {
            return Row(id: id, messageId: message.id, role: fromUser ? .user : .assistant,
                       kind: .image(projectId: projectId, ref: a.blobRef, fromUser: fromUser))
        }
        return Row(id: id, messageId: message.id, role: fromUser ? .user : .assistant,
                   kind: .file(name: a.name, subtitle: subtitle(for: a), projectId: projectId, ref: a.blobRef))
    }

    /// UTType-style labels some clients leave as the only text on a media-only send.
    private static func isGenericMediaLabel(_ text: String) -> Bool {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return t == "image" || t == "photo" || t == "video" || t == "attachment" || t == "(attachment)"
    }

    private static func subtitle(for a: Attachment) -> String {
        if let dot = a.name.lastIndex(of: ".") {
            return String(a.name[a.name.index(after: dot)...]).uppercased() + " file"
        }
        return a.mime
    }

    /// "Handed to subagent itsdangerous-tests" → "Messaged ◉ itsdangerous-tests".
    private static func handoffText(_ raw: String) -> String {
        let t = raw.trimmingCharacters(in: .whitespaces)
        for prefix in ["Handed to subagent ", "Handed to ", "Handed off to "] where t.hasPrefix(prefix) {
            return "Messaged \u{FFFC}" + t.dropFirst(prefix.count)
        }
        return t
    }

    /// Drop a legacy leading "@handle asked: …" line that older agentd builds wrote into prose.
    static func stripAskedPrefix(_ text: String) -> String {
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
        guard let first = lines.first,
              first.hasPrefix("@"),
              first.contains(" asked:") else { return text }
        var i = 1
        while i < lines.count && lines[i].isEmpty { i += 1 }
        return lines[i...].joined(separator: "\n")
    }

    static func dateLabel(_ ms: Int64) -> String {
        let date = Date(timeIntervalSince1970: Double(ms) / 1000)
        let cal = Calendar.current
        let time = date.formatted(date: .omitted, time: .shortened)
        if cal.isDateInToday(date) { return "Today \(time)" }
        if cal.isDateInYesterday(date) { return "Yesterday \(time)" }
        if let week = cal.date(byAdding: .day, value: -6, to: Date()), date > week {
            return date.formatted(.dateTime.weekday(.wide)) + " \(time)"
        }
        return date.formatted(date: .abbreviated, time: .shortened)
    }
}

/// Minimal block splitter: fences, headings, bullets, numbered lists, tables, paragraphs.
enum Markdown {
    /// Display polish for assistant bubbles already in the doc: insert a missing space
    /// after `.`/`!`/`?` when the next character is uppercase ("have.Checking" →
    /// "have. Checking"), between a lowercase run and an uppercase sentence start
    /// ("findingsDon't" → "findings Don't"), and drop a trailing short tool-narration
    /// sentence ("Checking the latest…") so the bubble stays prose-only.
    static func polishAssistantProse(_ text: String) -> String {
        fixGluedSentenceSpaces(stripTrailingToolNarration(text))
    }

    /// True when `chars[i]` (lowercase) is glued to an uppercase letter that starts a new
    /// sentence/word ("findingsDon't"), not mid-camelCase ("GitHub", "iPhone").
    private static func isGluedWordBreak(_ chars: [Character], _ i: Int) -> Bool {
        guard i + 1 < chars.count else { return false }
        let c = chars[i], next = chars[i + 1]
        guard c.isLowercase, next.isUppercase else { return false }
        let continues: Bool = {
            guard i + 2 < chars.count else { return true }
            let n = chars[i + 2]
            return n.isLowercase || n == "'"
        }()
        guard continues else { return false }
        var runStart = i
        while runStart > 0 && chars[runStart - 1].isLowercase { runStart -= 1 }
        if runStart > 0, chars[runStart - 1].isLetter { return false }
        return (i + 1 - runStart) >= 2
    }

    private static func fixGluedSentenceSpaces(_ text: String) -> String {
        let chars = Array(text)
        var out = String()
        out.reserveCapacity(text.count + 8)
        var i = 0
        while i < chars.count {
            let c = chars[i]
            out.append(c)
            if c == "." || c == "!" || c == "?" {
                let abbrev = c == "." && i >= 2 && chars[i - 1].isLowercase && chars[i - 2] == "."
                if !abbrev, i + 1 < chars.count, chars[i + 1].isUppercase {
                    out.append(" ")
                }
            } else if isGluedWordBreak(chars, i) {
                out.append(" ")
            }
            i += 1
        }
        return out
    }

    private static func stripTrailingToolNarration(_ text: String) -> String {
        let nl: String
        let body: String
        if text.hasSuffix("\r\n") {
            body = String(text.dropLast(2)); nl = "\r\n"
        } else if text.hasSuffix("\n") {
            body = String(text.dropLast()); nl = "\n"
        } else {
            body = text; nl = ""
        }
        var trimmed = body
        while trimmed.last?.isWhitespace == true { trimmed = String(trimmed.dropLast()) }
        if trimmed.isEmpty { return "" }
        let chars = Array(trimmed)
        var splitAt: Int? = nil
        var i = 0
        while i < chars.count {
            let c = chars[i]
            if c == "." || c == "!" || c == "?" {
                let abbrev = c == "." && i >= 2 && chars[i - 1].isLowercase && chars[i - 2] == "."
                if !abbrev {
                    let after = String(chars[(i + 1)...])
                    if after.isEmpty { break }
                    if after.hasPrefix(" ") || after.hasPrefix("\n") || after.hasPrefix("\r")
                        || after.first?.isUppercase == true {
                        splitAt = i + 1
                    }
                }
            }
            i += 1
        }
        guard let at = splitAt else {
            return isShortToolNarration(trimmed) ? "" : trimmed + nl
        }
        let head = String(chars[..<at]).trimmingCharacters(in: .whitespaces)
        let tail = String(chars[at...]).trimmingCharacters(in: .whitespaces)
        if isShortToolNarration(tail) {
            return head + nl
        }
        return trimmed + nl
    }

    private static func isShortToolNarration(_ sentence: String) -> Bool {
        var l = sentence.trimmingCharacters(in: .whitespacesAndNewlines)
        while let first = l.first, "*_#>- ".contains(first) { l = String(l.dropFirst()) }
        l = l.lowercased()
        if l.isEmpty || l.count > 90 { return false }
        let starters = [
            "running ", "executing ", "calling ", "invoking ",
            "let me check", "let me run", "let me look",
            "i'll check", "i will check", "i'll run", "i will run",
            "checking ", "reading the file",
            "first, i", "first i", "now i'll", "now i will",
        ]
        return starters.contains { l.hasPrefix($0) }
    }

    static func blocks(_ text: String) -> [Block] {
        var out: [Block] = []
        var paragraph: [String] = []
        var bullets: [String] = []
        var numbered: [String] = []
        var table: [String] = []
        var code: [String]? = nil
        var lang = ""

        func flush() {
            if !paragraph.isEmpty { out.append(.paragraph(paragraph.joined(separator: " "))); paragraph = [] }
            if !bullets.isEmpty { out.append(.bullets(bullets)); bullets = [] }
            if !numbered.isEmpty { out.append(.numbered(numbered)); numbered = [] }
            if !table.isEmpty { out.append(.table(table)); table = [] }
        }

        for rawLine in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(rawLine)
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if let c = code {
                if trimmed.hasPrefix("```") {
                    out.append(.code(lang: lang, code: c.joined(separator: "\n")))
                    code = nil
                } else {
                    code!.append(line)
                }
                continue
            }
            if trimmed.hasPrefix("```") {
                flush()
                lang = String(trimmed.dropFirst(3)).trimmingCharacters(in: .whitespaces)
                code = []
                continue
            }
            if trimmed.isEmpty { flush(); continue }
            if trimmed.hasPrefix("#") {
                flush()
                let level = trimmed.prefix { $0 == "#" }.count
                out.append(.heading(level: min(level, 3), text: trimmed.drop { $0 == "#" }.trimmingCharacters(in: .whitespaces)))
                continue
            }
            if trimmed.hasPrefix("- ") || trimmed.hasPrefix("* ") || trimmed.hasPrefix("• ") {
                if !paragraph.isEmpty || !numbered.isEmpty || !table.isEmpty { flush() }
                bullets.append(String(trimmed.dropFirst(2)))
                continue
            }
            if let dot = trimmed.firstIndex(of: "."), dot > trimmed.startIndex, trimmed[trimmed.startIndex..<dot].allSatisfy(\.isNumber),
               trimmed.distance(from: trimmed.startIndex, to: dot) <= 3, trimmed[dot...].hasPrefix(". ") {
                if !paragraph.isEmpty || !bullets.isEmpty || !table.isEmpty { flush() }
                numbered.append(trimmed[trimmed.index(dot, offsetBy: 2)...].trimmingCharacters(in: .whitespaces))
                continue
            }
            if trimmed.hasPrefix("|") {
                if !paragraph.isEmpty || !bullets.isEmpty || !numbered.isEmpty { flush() }
                if !trimmed.replacingOccurrences(of: "|", with: "").replacingOccurrences(of: "-", with: "").replacingOccurrences(of: ":", with: "").trimmingCharacters(in: .whitespaces).isEmpty {
                    table.append(trimmed)
                }
                continue
            }
            if !bullets.isEmpty || !numbered.isEmpty || !table.isEmpty { flush() }
            paragraph.append(trimmed)
        }
        if let c = code { out.append(.code(lang: lang, code: c.joined(separator: "\n"))) }
        flush()
        return out
    }

    /// A block that is really machine output (a JSON object/array) is not prose.
    static func isJSON(_ block: Block) -> Bool {
        let text: String
        switch block {
        case .paragraph(let t): text = t
        case .code(let lang, let c) where lang.isEmpty || lang.lowercased() == "json": text = c
        default: return false
        }
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard t.hasPrefix("{") || t.hasPrefix("["), let data = t.data(using: .utf8) else { return false }
        return (try? JSONSerialization.jsonObject(with: data)) != nil
    }

    /// Inline markdown with Mac-parity link paint and Grok-style code chips:
    /// accent + underline on links; monospaced (a point under body) on a soft
    /// wash for ``code``. Bare `http(s)://` URLs are linked too.
    static func inline(_ text: String, color: Color, link: Color, codeChip: Color) -> AttributedString {
        var attr = (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(text)
        linkBareURLs(&attr)
        for run in attr.runs {
            let range = run.range
            if run.link != nil {
                attr[range].swiftUI.foregroundColor = link
                attr[range].underlineStyle = .single
            } else {
                attr[range].swiftUI.foregroundColor = color
            }
            // Default markdown mono ignores the bubble's 17 pt body and looks
            // undersized next to SF Pro; pin size and chip like Mac / M-101.
            if run.inlinePresentationIntent?.contains(.code) == true {
                attr[range].swiftUI.font = .system(size: Theme.code, design: .monospaced)
                attr[range].swiftUI.backgroundColor = codeChip
            }
        }
        return attr
    }

    private static let linkDetector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)

    private static func linkBareURLs(_ attr: inout AttributedString) {
        guard let detector = linkDetector else { return }
        let plain = String(attr.characters)
        let ns = plain as NSString
        for match in detector.matches(in: plain, range: NSRange(location: 0, length: ns.length)) {
            guard let url = match.url,
                  let scheme = url.scheme?.lowercased(),
                  scheme == "http" || scheme == "https",
                  let range = Range(match.range, in: attr) else { continue }
            if attr[range].link == nil {
                attr[range].link = url
            }
        }
    }
}

// MARK: - Row views

struct RowView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @ObservedObject private var accents = SenderAccents.shared
    let row: Row
    let chatId: Id
    let width: CGFloat
    /// How far the transcript has been dragged left to reveal iMessage-style times (0…`timeRevealWidth`).
    var timeReveal: CGFloat = 0
    /// A tap on a reply bubble's quote header: the chat scrolls to this message id.
    var onQuoteTap: ((Id) -> Void)? = nil
    /// Briefly tinted after a quote tap landed on this row's message.
    var highlighted: Bool = false

    /// Trailing column width for swipe-revealed times (fits "12:59 PM").
    static let timeRevealWidth: CGFloat = 56

    private var maxBubble: CGFloat { max(200, width - Theme.bubbleInset) }

    private var reportsGreetingFrame: Bool {
        model.firstLogin != nil && model.messages(of: chatId).first(where: { $0.greeting })?.id == row.messageId
    }

    /// Bubbles get a swipe time; centred system / card / error rows do not.
    private var showsSwipeTime: Bool {
        switch row.kind {
        case .system, .question, .error:
            return false
        case .user, .other, .member, .assistant, .file, .voiceMemo, .image:
            return true
        }
    }

    private var swipeTimeLabel: String? {
        guard showsSwipeTime else { return nil }
        guard let ms = model.messages(of: chatId).first(where: { $0.id == row.messageId })?.createdAt,
              ms > 0 else { return nil }
        return Date(timeIntervalSince1970: Double(ms) / 1000)
            .formatted(date: .omitted, time: .shortened)
    }

    var body: some View {
        VStack(spacing: 4) {
            bubble
            if !row.reactions.isEmpty {
                reactionChips
            }
        }
        // Quote tap landed here: a soft accent wash that fades out (Mac parity).
        .background(
            RoundedRectangle(cornerRadius: Theme.bubbleRadius + 4, style: .continuous)
                .fill(theme.accent.opacity(highlighted ? 0.14 : 0))
                .padding(-4)
        )
        .animation(.easeInOut(duration: 0.35), value: highlighted)
        // iMessage: drag left → whole row slides; time sits on the trailing edge.
        .offset(x: -timeReveal)
        .overlay(alignment: .trailing) {
            if let time = swipeTimeLabel, timeReveal > 0.5 {
                Text(time)
                    .font(theme.font(11))
                    .foregroundStyle(theme.muted)
                    .frame(width: Self.timeRevealWidth, alignment: .trailing)
                    .offset(x: Self.timeRevealWidth - timeReveal)
                    .opacity(Double(min(1, timeReveal / (Self.timeRevealWidth * 0.55))))
                    .allowsHitTesting(false)
                    .accessibilityHidden(timeReveal < 12)
            }
        }
    }

    @ViewBuilder
    private var bubble: some View {
        switch row.kind {
        case .user(let text, let pending, let quote):
            HStack {
                Spacer(minLength: 0)
                VStack(alignment: .leading, spacing: 6) {
                    if let quote {
                        quoteHeader(quote, color: theme.userText)
                    }
                    Text(Markdown.inline(text, color: theme.userText, link: theme.accent, codeChip: theme.userCodeChip))
                        .font(theme.font(Theme.body))
                        .lineSpacing(Theme.bodyLineSpacing)
                        .tint(theme.accent)
                        .textSelection(.enabled)
                }
                .padding(.horizontal, Theme.bubbleHPad)
                .padding(.vertical, Theme.bubbleVPad)
                .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleUser))
                .frame(maxWidth: maxBubble, alignment: .trailing)
                .opacity(pending ? 0.6 : 1)
            }
        case .other(let text, let handle, let avatar, let named, let quote):
            HStack(alignment: .bottom, spacing: 8) {
                Button { model.openPersonProfile(handle: handle, avatarUrl: avatar, chatId: chatId) } label: {
                    PersonAvatar(url: avatar, handle: handle, size: 26, name: model.personName(handle: handle), id: model.personId(handle: handle))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("@\(handle), profile")
                VStack(alignment: .leading, spacing: 3) {
                    if named {
                        Button { model.openPersonProfile(handle: handle, avatarUrl: avatar, chatId: chatId) } label: {
                            Text(model.personName(handle: handle) ?? "@\(handle)")
                                .font(theme.font(Theme.tiny, weight: .medium))
                                .foregroundStyle(theme.muted)
                                .padding(.leading, 4)
                        }
                        .buttonStyle(.plain)
                    }
                    VStack(alignment: .leading, spacing: 6) {
                        if let quote {
                            quoteHeader(quote, color: theme.text)
                        }
                        Text(Markdown.inline(text, color: theme.text, link: theme.accent, codeChip: theme.codeChip))
                            .font(theme.font(Theme.body))
                            .lineSpacing(Theme.bodyLineSpacing)
                            .tint(theme.accent)
                            .textSelection(.enabled)
                    }
                    .padding(.horizontal, Theme.bubbleHPad)
                    .padding(.vertical, Theme.bubbleVPad)
                    .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleAssistant))
                    .senderBar(personAccent(handle: handle, avatar: avatar))
                }
                .frame(maxWidth: maxBubble - 34, alignment: .leading)
                Spacer(minLength: 0)
            }
            .task(id: avatar) { accents.load(url: avatar) }
        case .member(let blocks, let live, let handle, let avatar, let named, let agentId):
            let openMember = {
                if let agentId { model.showProfile(.agent(projectId: agentId)) }
                else { model.openPersonProfile(handle: handle, avatarUrl: avatar, chatId: chatId) }
            }
            HStack(alignment: .bottom, spacing: 8) {
                Button(action: openMember) { peopleAvatar(handle: handle, avatar: avatar) }
                    .buttonStyle(.plain)
                    .accessibilityLabel("@\(handle), profile")
                VStack(alignment: .leading, spacing: 3) {
                    if named {
                        Button(action: openMember) {
                            Text(memberProject(handle: handle)?.name ?? agentId.flatMap { model.foreignAgent($0)?.name } ?? "@\(handle)")
                                .font(theme.font(Theme.tiny, weight: .medium))
                                .foregroundStyle(theme.muted)
                                .padding(.leading, 4)
                        }
                        .buttonStyle(.plain)
                    }
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(Array(blocks.enumerated()), id: \.offset) { i, block in
                            BlockView(block: block, live: live && i == blocks.count - 1)
                        }
                    }
                    .padding(.horizontal, Theme.bubbleHPad)
                    .padding(.vertical, Theme.bubbleVPad)
                    .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleAssistant))
                    .senderBar(memberAccent(handle: handle, avatar: avatar))
                }
                .frame(maxWidth: maxBubble - 34, alignment: .leading)
                Spacer(minLength: 0)
            }
            .task(id: avatar) { if memberProject(handle: handle) == nil { accents.load(url: avatar) } }
        case .assistant(let blocks, let live, let caption):
            HStack {
                VStack(alignment: .leading, spacing: 8) {
                    if let caption {
                        Text(caption)
                            .font(theme.font(Theme.tiny, weight: .medium))
                            .foregroundStyle(theme.muted)
                    }
                    ForEach(Array(blocks.enumerated()), id: \.offset) { i, block in
                        BlockView(block: block, live: live && i == blocks.count - 1)
                    }
                }
                .padding(.horizontal, Theme.bubbleHPad)
                .padding(.vertical, Theme.bubbleVPad)
                .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleAssistant))
                .background {
                    // D82: while the first-login terminal morphs into this bubble it
                    // needs the bubble's exact frame; only the greeting row reports.
                    if reportsGreetingFrame {
                        GeometryReader { g in
                            Color.clear
                                .onAppear { model.greetingBubbleFrame = g.frame(in: .global) }
                                .onChange(of: g.frame(in: .global)) { _, f in model.greetingBubbleFrame = f }
                        }
                    }
                }
                .frame(maxWidth: maxBubble, alignment: .leading)
                Spacer(minLength: 0)
            }
        case .system(let icon, let text, let avatarProject):
            HStack(spacing: 5) {
                if let icon {
                    Image(systemName: icon).font(.system(size: 13, weight: .regular))
                }
                systemText(text, avatarProject: avatarProject)
            }
            .font(theme.font(Theme.small))
            .foregroundStyle(theme.muted)
            .frame(maxWidth: .infinity)
            .multilineTextAlignment(.center)
        case .question(let questionId, let text, let options, let multi, let picked, let open):
            HStack {
                QuestionCard(rowId: row.id, chatId: chatId, questionId: questionId, title: text, options: options,
                             multi: multi, picked: picked, open: open)
                    .frame(maxWidth: maxBubble, alignment: .leading)
                Spacer(minLength: 0)
            }
        case .file(let name, let subtitle, let projectId, let ref):
            HStack {
                if row.role == .user { Spacer(minLength: 0) }
                FileCard(name: name, subtitle: subtitle, projectId: projectId, blobRef: ref, fromUser: row.role == .user)
                    .frame(maxWidth: maxBubble, alignment: row.role == .user ? .trailing : .leading)
                if row.role != .user { Spacer(minLength: 0) }
            }
        case .voiceMemo(let name, let projectId, let ref, let fromUser):
            HStack {
                if fromUser { Spacer(minLength: 0) }
                VoiceMemoBubble(name: name, projectId: projectId, blobRef: ref, fromUser: fromUser)
                    .frame(maxWidth: maxBubble, alignment: fromUser ? .trailing : .leading)
                if !fromUser { Spacer(minLength: 0) }
            }
        case .image(let projectId, let ref, let fromUser):
            HStack {
                if fromUser { Spacer(minLength: 0) }
                ImageBubble(projectId: projectId, blobRef: ref, fromUser: fromUser, maxWidth: maxBubble)
                if !fromUser { Spacer(minLength: 0) }
            }
        case .error(let code, let message):
            HStack(spacing: 10) {
                Image(systemName: "exclamationmark.circle").foregroundStyle(theme.danger)
                Text(message).font(theme.font(Theme.small)).foregroundStyle(theme.text)
                Spacer(minLength: 0)
                Text(code).font(theme.font(11)).foregroundStyle(theme.muted)
                Button("Retry") { model.retry(chatId: chatId, messageId: row.messageId) }
                    .font(theme.font(13, weight: .semibold))
                    .foregroundStyle(theme.text)
            }
            .padding(.horizontal, Theme.bubbleHPad).padding(.vertical, 10)
            .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.dangerSoft))
        }
    }

    /// Compact chips under the bubble (iMessage-style). Tap toggles your own reaction.
    private var reactionChips: some View {
        let mine = model.myUserId
        return HStack(spacing: 4) {
            if row.role == .user { Spacer(minLength: 0) }
            ForEach(row.reactions, id: \.emoji) { reaction in
                let mineOn = mine.map { reaction.senders.contains($0) } ?? false
                Button {
                    model.toggleReaction(chatId: chatId, messageId: row.messageId, emoji: reaction.emoji)
                } label: {
                    HStack(spacing: 3) {
                        Text(reaction.emoji).font(theme.font(14))
                        if reaction.senders.count > 1 {
                            Text("\(reaction.senders.count)")
                                .font(theme.font(12, weight: .medium))
                                .foregroundStyle(theme.muted)
                        }
                    }
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(
                        Capsule(style: .continuous)
                            .fill(mineOn ? theme.accent.opacity(0.22) : theme.card)
                    )
                    .overlay(
                        Capsule(style: .continuous)
                            .strokeBorder(mineOn ? theme.accent.opacity(0.55) : theme.separator, lineWidth: 1)
                    )
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(reaction.emoji) reaction, \(reaction.senders.count)")
            }
            if row.role != .user { Spacer(minLength: 0) }
        }
        .padding(.leading, reactionLeading)
    }

    /// Match the avatar offset used by people-chat bubbles (26 pt + 8 pt gap).
    private var reactionLeading: CGFloat {
        switch row.kind {
        case .other, .member: return 34
        default: return 0
        }
    }

    /// A people-chat member handle that is one of my agents.
    private func memberProject(handle: String) -> Project? {
        model.projects.first { mentionSlug($0.name).caseInsensitiveCompare(handle) == .orderedSame }
    }

    /// Avatar for a people-chat agent: project tree when we know it, else sender slug / initials.
    @ViewBuilder
    private func peopleAvatar(handle: String, avatar: String?) -> some View {
        if let project = memberProject(handle: handle) {
            AgentAvatar(project: project, theme: theme, size: 26, still: true)
        } else if let avatar, avatar.contains("://") {
            PersonAvatar(url: avatar, handle: handle, size: 26, name: model.personName(handle: handle), id: model.personId(handle: handle))
        } else {
            PersonAvatar(url: nil, handle: handle, size: 26, name: model.personName(handle: handle), id: model.personId(handle: handle))
        }
    }

    /// The bar colour on a person's bubble: their picture's dominant colour,
    /// else the D79 colour of the same disc `PersonAvatar` draws (`id ?? handle`).
    private func personAccent(handle: String, avatar: String?) -> Color {
        accents.color(url: avatar, seed: model.personId(handle: handle) ?? handle, dark: theme.isDark)
    }

    /// Mac parity: an agent's bubbles in a people chat carry its disc colour
    /// (the leaf's disc under D92, the D79 gradient midpoint otherwise).
    private func memberAccent(handle: String, avatar: String?) -> Color {
        if let project = memberProject(handle: handle) {
            if LeafAvatarsFlag.shared.on { return LeafArt.accent(project, dark: theme.isDark) }
            return accents.color(url: nil, seed: project.id, override: project.effectiveAvatarColorIndex, dark: theme.isDark)
        }
        return personAccent(handle: handle, avatar: avatar)
    }

    /// Grok Bot's quote header at the top of a reply bubble: speech-bubble glyph, then the
    /// original's first words in quotes on one line. Tap → the chat scrolls to the original.
    private func quoteHeader(_ quote: Quote, color: Color) -> some View {
        Button {
            onQuoteTap?(quote.messageId)
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "bubble.left")
                    .font(.system(size: 11, weight: .medium))
                Text("“\(quote.text)”")
                    .font(theme.font(Theme.small))
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            .foregroundStyle(color.opacity(0.72))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Replying to: \(quote.text)")
    }

    /// "Messaged \u{FFFC}Affine" / "6 messages with \u{FFFC}A, \u{FFFC}B and \u{FFFC}C":
    /// every object marker becomes a small avatar before the name that follows it.
    @ViewBuilder
    private func systemText(_ text: String, avatarProject: Id?) -> some View {
        if text.contains("\u{FFFC}") {
            let pieces = Self.splitMarkers(text)
            HStack(spacing: 5) {
                ForEach(Array(pieces.enumerated()), id: \.offset) { _, piece in
                    if piece.isName {
                        agentGlyph(named: piece.text)
                        Text(piece.text)
                    } else {
                        Text(piece.text)
                    }
                }
            }
            .lineLimit(1)
            .minimumScaleFactor(0.85)
        } else {
            Text(text)
        }
    }

    @ViewBuilder
    private func agentGlyph(named name: String) -> some View {
        if let target = model.projects.first(where: { $0.name.caseInsensitiveCompare(name) == .orderedSame }) {
            AgentAvatar(project: target, theme: theme, size: 16, still: true)
        } else {
            // An agent we do not list: initials on the colour its name hashes to.
            InitialsAvatar(name: name, handle: name, id: name, size: 16)
        }
    }

    /// Split "6 messages with ◉Kenny, ◉Tyler and ◉Jenny" into plain text and name pieces;
    /// a name runs from its marker to the next ", " or " and ".
    static func splitMarkers(_ text: String) -> [(text: String, isName: Bool)] {
        var out: [(String, Bool)] = []
        let parts = text.components(separatedBy: "\u{FFFC}")
        if let head = parts.first, !head.isEmpty { out.append((head.trimmingCharacters(in: .whitespaces), false)) }
        for seg in parts.dropFirst() {
            var name = seg
            var rest = ""
            for sep in [", ", " and "] {
                if let r = seg.range(of: sep), r.lowerBound < name.endIndex {
                    let candidate = String(seg[..<r.lowerBound])
                    if candidate.count < name.count { name = candidate; rest = String(seg[r.lowerBound...]) }
                }
            }
            out.append((name, true))
            let trimmed = rest.trimmingCharacters(in: .whitespaces)
            if !trimmed.isEmpty { out.append((trimmed, false)) }
        }
        return out
    }
}

struct BlockView: View {
    @Environment(\.theme) private var theme
    let block: Block
    let live: Bool

    var body: some View {
        switch block {
        case .paragraph(let text):
            if live {
                StreamingText(text: text, font: .system(size: Theme.body), color: theme.text)
                    .lineSpacing(Theme.bodyLineSpacing)
            } else {
                Text(Markdown.inline(text, color: theme.text, link: theme.accent, codeChip: theme.codeChip))
                    .font(theme.font(Theme.body))
                    .lineSpacing(Theme.bodyLineSpacing)
                    .tint(theme.accent)
                    .textSelection(.enabled)
            }
        case .heading(_, let text):
            Text(Markdown.inline(text, color: theme.text, link: theme.accent, codeChip: theme.codeChip))
                .font(theme.font(Theme.body, weight: .semibold))
                .tint(theme.accent)
        case .bullets(let items), .numbered(let items):
            let numbered: Bool = { if case .numbered = block { return true } else { return false } }()
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(items.enumerated()), id: \.offset) { i, item in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(numbered ? "\(i + 1)." : "•")
                            .font(theme.font(Theme.body))
                            .foregroundStyle(theme.text)
                            .frame(minWidth: 16, alignment: .trailing)
                        if live && i == items.count - 1 {
                            StreamingText(text: item, font: .system(size: Theme.body), color: theme.text)
                        } else {
                            Text(Markdown.inline(item, color: theme.text, link: theme.accent, codeChip: theme.codeChip))
                                .font(theme.font(Theme.body))
                                .tint(theme.accent)
                        }
                    }
                }
            }
        case .table(let lines):
            ScrollView(.horizontal, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(Array(lines.enumerated()), id: \.offset) { i, line in
                        Text(line.split(separator: "|").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }.joined(separator: "    "))
                            .font(theme.font(14, weight: i == 0 ? .semibold : .regular, design: .monospaced))
                            .foregroundStyle(theme.text)
                    }
                }
                .padding(10)
            }
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(theme.bg.opacity(0.6)))
        case .code(_, let code):
            ScrollView(.horizontal, showsIndicators: false) {
                Text(code)
                    .font(theme.font(14, design: .monospaced))
                    .foregroundStyle(theme.text)
                    .textSelection(.enabled)
                    .padding(10)
            }
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(theme.bg.opacity(0.6)))
        }
    }
}

/// The 2 pt coloured sliver on the left edge of a sender's bubble (the Mac's
/// `border_l_2` at 70 %), clipped to the bubble's corners.
private extension View {
    func senderBar(_ color: Color) -> some View {
        overlay(alignment: .leading) {
            Rectangle().fill(color.opacity(0.7)).frame(width: 2)
        }
        .clipShape(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous))
    }
}

/// Bubble-bar colours per sender, the same arithmetic as the Mac through
/// ArbosFFI (`arbos_core::avatar`): a picture's dominant vibrant colour,
/// extracted once per avatar URL per launch, else the midpoint of the D79
/// disc gradient. A colourless picture (black-and-white, logo on white) gets
/// no entry and falls back like no picture.
final class SenderAccents: ObservableObject {
    static let shared = SenderAccents()

    @Published private(set) var byUrl: [String: AccentHex] = [:]
    /// Touched from the main thread only (`load` runs from `.task`).
    private var requested: Set<String> = []

    /// The bar colour for the current appearance.
    func color(url: String?, seed: String, override: Int? = nil, dark: Bool) -> Color {
        let accent: AccentHex
        if let url, let known = byUrl[url] {
            accent = known
        } else {
            accent = avatarFallbackAccent(seed: seed, overrideIndex: override.map { UInt8(clamping: $0) })
        }
        return Color(uiColor: UIColor(hexString: dark ? accent.dark : accent.light) ?? .systemGray)
    }

    /// Fetch (through the shared URLCache, so usually no second download after
    /// `AsyncImage`) and extract; repeated calls for one URL are free.
    func load(url: String?) {
        guard let url, url.contains("://"), let u = URL(string: url), !requested.contains(url) else { return }
        requested.insert(url)
        Task.detached(priority: .utility) { [weak self] in
            guard let (data, _) = try? await URLSession.shared.data(from: u),
                  let accent = Self.extract(data) else { return }
            await MainActor.run { self?.byUrl[url] = accent }
        }
    }

    /// 96 × 96 RGBA into the shared extractor (which box-averages to 48 × 48 itself).
    private static func extract(_ data: Data) -> AccentHex? {
        guard let cg = UIImage(data: data)?.cgImage else { return nil }
        let side = 96
        var pixels = [UInt8](repeating: 0, count: side * side * 4)
        let info = CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
        let drawn = pixels.withUnsafeMutableBytes { buf -> Bool in
            guard let ctx = CGContext(data: buf.baseAddress, width: side, height: side, bitsPerComponent: 8,
                                      bytesPerRow: side * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: info)
            else { return false }
            ctx.interpolationQuality = .high
            ctx.draw(cg, in: CGRect(x: 0, y: 0, width: side, height: side))
            return true
        }
        guard drawn else { return nil }
        return avatarDominantAccent(rgba: Data(pixels), width: UInt32(side), height: UInt32(side))
    }
}

/// Round avatar for a person: the GitHub/uploaded picture when there is one,
/// else the Telegram-style initials circle (D79). `id` is the stable user id
/// the colour is derived from; without it the handle stands in.
struct PersonAvatar: View {
    @Environment(\.theme) private var theme
    let url: String?
    let handle: String
    var size: CGFloat = 40
    var name: String? = nil
    var id: String? = nil
    var colorIndex: Int? = nil

    var body: some View {
        ZStack {
            initials
            if let url, let u = URL(string: url) {
                AsyncImage(url: u) { img in img.resizable().scaledToFill() } placeholder: { initials }
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
    }

    private var initials: some View {
        InitialsAvatar(name: name, handle: handle, id: id ?? handle, override: colorIndex, size: size)
    }
}

/// Up to three overlapping member pictures, then a plus when there are more.
/// A custom `imageUrl` (group picture) replaces the stack.
struct AvatarCluster: View {
    @Environment(\.theme) private var theme
    let members: [ChatMember]
    var size: CGFloat = Theme.avatar
    var maxVisible: Int = 3
    var imageUrl: String? = nil
    /// D92 amendment: the group's leaf picture from the Create screen (a custom picture wins).
    var leaf: LeafPick? = nil

    private var shown: [ChatMember] { Array(members.prefix(maxVisible)) }
    private var hasMore: Bool { members.count > maxVisible }

    var body: some View {
        if let leaf, imageUrl?.isEmpty ?? true, LeafAvatarsFlag.shared.on {
            LeafAvatar(pick: leaf, size: size * 1.12)
        } else if let imageUrl, !imageUrl.isEmpty {
            // Keep the group silhouette under the picture until it arrives so we
            // do not flash initials ("G") then flip to the selected image.
            ZStack {
                Circle().fill(theme.card)
                Image(systemName: "person.2.fill")
                    .font(theme.font(size * 0.4, weight: .medium))
                    .foregroundStyle(theme.accent)
                if let u = URL(string: imageUrl) {
                    AsyncImage(url: u) { phase in
                        switch phase {
                        case .success(let img):
                            img.resizable().scaledToFill()
                        case .empty, .failure:
                            Color.clear
                        @unknown default:
                            Color.clear
                        }
                    }
                }
            }
            .frame(width: size, height: size)
            .clipShape(Circle())
        } else if shown.isEmpty {
            GlassCircle(size: size) {
                Image(systemName: "person.2.fill")
                    .font(theme.font(size * 0.4))
                    .foregroundStyle(theme.text)
            }
        } else {
            HStack(spacing: -size * 0.32) {
                ForEach(Array(shown.enumerated()), id: \.element.id) { _, m in
                    PersonAvatar(url: m.avatarUrl, handle: m.handle, size: size, name: m.name, id: m.id, colorIndex: m.avatarColorIndex)
                        .overlay(Circle().strokeBorder(theme.bg, lineWidth: max(1, size * 0.06)))
                }
                if hasMore {
                    ZStack {
                        Circle().fill(theme.field)
                        Image(systemName: "plus")
                            .font(theme.font(size * 0.35, weight: .bold))
                            .foregroundStyle(theme.text)
                    }
                    .frame(width: size * 0.85, height: size * 0.85)
                    .overlay(Circle().strokeBorder(theme.bg, lineWidth: max(1, size * 0.06)))
                }
            }
        }
    }
}

/// Inline image attachment: loads with the device bearer (same as voice memos),
/// tappable for a full-screen look. AsyncImage alone cannot send `Authorization`
/// and often stays blank on people-chat blob URLs.
struct ImageBubble: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let projectId: Id
    let blobRef: String?
    let fromUser: Bool
    let maxWidth: CGFloat

    @State private var image: UIImage?
    @State private var failed = false
    @State private var showFull = false

    private var width: CGFloat { min(240, maxWidth) }
    private var fill: Color { fromUser ? theme.bubbleUser : theme.bubbleAssistant }

    var body: some View {
        Button { if image != nil { showFull = true } } label: {
            ZStack {
                fill
                if let image {
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFill()
                } else if failed || blobRef == nil {
                    Image(systemName: "photo")
                        .font(theme.font(28))
                        .foregroundStyle(fromUser ? theme.userText.opacity(0.7) : theme.muted)
                } else {
                    ProgressView().tint(fromUser ? theme.userText.opacity(0.7) : theme.muted)
                }
            }
            .frame(width: width, height: 180)
            .clipShape(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous))
        }
        .buttonStyle(.plain)
        .disabled(image == nil)
        .accessibilityLabel(image == nil ? "Image" : "Image, tap to view")
        .task(id: blobRef) { await load() }
        .fullScreenCover(isPresented: $showFull) {
            if let image {
                ZoomableImageViewer(image: image, onClose: { showFull = false })
            }
        }
    }

    private func load() async {
        guard let ref = blobRef,
              let url = model.blobURL(projectId: projectId, key: ref) else {
            failed = true
            return
        }
        var req = URLRequest(url: url)
        for (k, v) in model.authHeaders { req.setValue(v, forHTTPHeaderField: k) }
        do {
            let (data, response) = try await URLSession.shared.data(for: req)
            guard let http = response as? HTTPURLResponse, (200..<400).contains(http.statusCode),
                  let img = UIImage(data: data) else {
                failed = true
                return
            }
            image = img
            failed = false
        } catch {
            failed = true
        }
    }
}

/// Full-screen image: pinch to zoom, drag to pan when zoomed, double-tap to toggle.
/// At 1×: swipe down or swipe right from the left edge to dismiss.
private struct ZoomableImageViewer: View {
    @Environment(\.theme) private var theme
    let image: UIImage
    let onClose: () -> Void

    @State private var scale: CGFloat = 1
    @State private var baseScale: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var baseOffset: CGSize = .zero

    private let minScale: CGFloat = 1
    private let maxScale: CGFloat = 5
    private let doubleTapScale: CGFloat = 2.5
    /// Vertical drag past this distance dismisses when not zoomed.
    private let dismissDownThreshold: CGFloat = 120
    /// Horizontal drag past this distance dismisses from the left edge.
    private let dismissEdgeThreshold: CGFloat = 100
    /// Start must be within this many points of the leading edge for edge swipe.
    private let edgeSwipeWidth: CGFloat = 28

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Color.black.opacity(backdropOpacity).ignoresSafeArea()
            Image(uiImage: image)
                .resizable()
                .scaledToFit()
                .scaleEffect(scale * dismissScale)
                .offset(offset)
                .ignoresSafeArea()
                .gesture(magnifyGesture)
                .simultaneousGesture(panGesture)
                .onTapGesture(count: 2, perform: toggleZoom)
                .accessibilityLabel("Image")
                .accessibilityHint("Pinch to zoom, double tap to zoom in or out, swipe down or from the left edge to close")
            Button(action: onClose) {
                Image(systemName: "xmark")
                    .font(theme.font(15, weight: .semibold))
                    .foregroundStyle(.white)
                    .frame(width: 36, height: 36)
                    .background(Circle().fill(Color.white.opacity(0.18)))
            }
            .buttonStyle(.plain)
            .padding(16)
            .accessibilityLabel("Close")
        }
    }

    /// Fade the black backdrop while the user drags to dismiss at 1×.
    private var backdropOpacity: Double {
        guard scale <= minScale + 0.01 else { return 1 }
        let distance = max(offset.height, offset.width)
        guard distance > 0 else { return 1 }
        return max(0.35, 1 - Double(distance) / 400)
    }

    /// Slight shrink while dragging to dismiss, matching the Photos-style feel.
    private var dismissScale: CGFloat {
        guard scale <= minScale + 0.01 else { return 1 }
        let distance = max(offset.height, offset.width)
        guard distance > 0 else { return 1 }
        return max(0.85, 1 - distance / 1000)
    }

    private var magnifyGesture: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                scale = min(max(baseScale * value.magnification, minScale), maxScale)
            }
            .onEnded { _ in
                baseScale = scale
                if scale <= minScale + 0.01 {
                    withAnimation(.easeOut(duration: 0.2)) { reset() }
                }
            }
    }

    private var panGesture: some Gesture {
        DragGesture()
            .onChanged { value in
                if scale > minScale {
                    offset = CGSize(
                        width: baseOffset.width + value.translation.width,
                        height: baseOffset.height + value.translation.height
                    )
                    return
                }
                // At 1×: follow a dismiss drag (down anywhere, or right from the left edge).
                let t = value.translation
                if t.height > 0, t.height >= abs(t.width) {
                    offset = CGSize(width: 0, height: t.height)
                } else if value.startLocation.x <= edgeSwipeWidth,
                          t.width > 0,
                          t.width >= abs(t.height) {
                    offset = CGSize(width: t.width, height: 0)
                }
            }
            .onEnded { value in
                if scale > minScale {
                    baseOffset = offset
                    return
                }
                let t = value.translation
                let dismissDown = t.height > dismissDownThreshold
                    && t.height > abs(t.width) * 1.5
                let dismissEdge = value.startLocation.x <= edgeSwipeWidth
                    && t.width > dismissEdgeThreshold
                    && t.width > abs(t.height) * 1.25
                if dismissDown || dismissEdge {
                    onClose()
                } else {
                    withAnimation(.easeOut(duration: 0.2)) {
                        offset = .zero
                        baseOffset = .zero
                    }
                }
            }
    }

    private func toggleZoom() {
        withAnimation(.easeOut(duration: 0.2)) {
            if scale > minScale {
                reset()
            } else {
                scale = doubleTapScale
                baseScale = doubleTapScale
            }
        }
    }

    private func reset() {
        scale = minScale
        baseScale = minScale
        offset = .zero
        baseOffset = .zero
    }
}

/// Document card (Grok: icon, title, domain) for a non-audio attachment.
/// Tap downloads the blob with the device bearer (Safari cannot send auth) and
/// opens a Quick Look preview so the user can read the file in-app.
struct FileCard: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let name: String
    let subtitle: String
    let projectId: Id
    let blobRef: String?
    let fromUser: Bool

    @State private var previewURL: URL?
    @State private var loading = false

    var body: some View {
        Button {
            Task { await open() }
        } label: {
            // Grok's file card (web-08, measured): 61 pt tall, 20×27 icon at 15 pt,
            // text 16 pt after the icon, 17 pt title over a 15 pt muted subtitle.
            HStack(spacing: 14) {
                RoundedRectangle(cornerRadius: 5, style: .continuous)
                    .fill(Color(hex: "#FFB000")!)
                    .frame(width: 21, height: 27)
                    .overlay(Image(systemName: "doc.text").font(.system(size: 12, weight: .bold)).foregroundStyle(.black))
                VStack(alignment: .leading, spacing: 2) {
                    Text(name).font(theme.font(Theme.body)).foregroundStyle(fromUser ? theme.userText : theme.text).lineLimit(1)
                    Text(subtitle).font(theme.font(Theme.small)).foregroundStyle(fromUser ? theme.userText.opacity(0.7) : theme.muted).lineLimit(1)
                }
                if loading {
                    ProgressView()
                        .tint(fromUser ? theme.userText.opacity(0.7) : theme.muted)
                        .controlSize(.small)
                }
            }
            .padding(.leading, 15)
            .padding(.trailing, Theme.bubbleHPad + 4)
            .padding(.vertical, 13)
            .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(fromUser ? theme.bubbleUser : theme.bubbleAssistant))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(blobRef == nil || loading)
        .accessibilityLabel(blobRef == nil ? "\(name), unavailable" : "\(name), \(subtitle), tap to open")
        .quickLookPreview($previewURL)
    }

    @MainActor
    private func open() async {
        // Already on disk from an earlier tap — re-present Quick Look.
        if let previewURL {
            present(previewURL)
            return
        }
        guard let ref = blobRef,
              let url = model.blobURL(projectId: projectId, key: ref) else { return }
        loading = true
        defer { loading = false }
        var req = URLRequest(url: url)
        for (k, v) in model.authHeaders { req.setValue(v, forHTTPHeaderField: k) }
        do {
            let (data, response) = try await URLSession.shared.data(for: req)
            guard let http = response as? HTTPURLResponse, (200..<400).contains(http.statusCode), !data.isEmpty else {
                return
            }
            let dir = FileManager.default.temporaryDirectory.appendingPathComponent("arbos-files", isDirectory: true)
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let safe = (name as NSString).lastPathComponent
            let file = dir.appendingPathComponent(safe.isEmpty ? "file" : safe)
            try data.write(to: file, options: .atomic)
            present(file)
        } catch {
            // Leave the card tappable so the user can retry.
        }
    }

    /// `.quickLookPreview` only presents when the binding becomes non-nil, so a
    /// second tap after dismiss must briefly clear it first.
    @MainActor
    private func present(_ url: URL) {
        if previewURL == url {
            previewURL = nil
            Task { @MainActor in previewURL = url }
        } else {
            previewURL = url
        }
    }
}

/// Voice memo bubble: play/pause, a static waveform, duration.
/// The whole chip is the control (waveform and timer included) — same idea as
/// `FileCard`. Audio is fetched with `URLSession` then played via
/// `AVAudioPlayer` so auth and the post-record `AVAudioSession` work;
/// streaming the blob through `AVPlayer` left the chip stuck on pause at `0:00`.
struct VoiceMemoBubble: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let name: String
    let projectId: Id
    let blobRef: String?
    let fromUser: Bool
    @StateObject private var player = MemoPlayer()

    var body: some View {
        Button {
            guard let ref = blobRef, let url = model.blobURL(projectId: projectId, key: ref) else { return }
            player.toggle(url: url, headers: model.authHeaders)
        } label: {
            HStack(spacing: 10) {
                Circle().fill(theme.primary).frame(width: 32, height: 32)
                    .overlay(Image(systemName: player.playing ? "pause.fill" : "play.fill").font(.system(size: 13, weight: .bold)).foregroundStyle(theme.onPrimary).offset(x: player.playing ? 0 : 1))
                HStack(spacing: 2) {
                    ForEach(0..<22, id: \.self) { i in
                        let played: Bool = Double(i) / 22.0 < player.progress
                        let barHeight: CGFloat = 6 + CGFloat((i * 7) % 13)
                        Capsule()
                            .fill((fromUser ? theme.userText : theme.text).opacity(played ? 1 : 0.35))
                            .frame(width: 3, height: barHeight)
                    }
                }
                Text(player.durationLabel)
                    .font(theme.font(Theme.tiny, design: .monospaced))
                    .foregroundStyle(fromUser ? theme.userText.opacity(0.8) : theme.muted)
            }
            .padding(.horizontal, Theme.bubbleHPad)
            .padding(.vertical, Theme.bubbleVPad)
            .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(fromUser ? theme.bubbleUser : theme.bubbleAssistant))
        }
        .buttonStyle(.plain)
        .disabled(blobRef == nil)
        .onAppear {
            if let ref = blobRef, let url = model.blobURL(projectId: projectId, key: ref) {
                player.loadDuration(url: url, headers: model.authHeaders)
            }
        }
    }
}

@MainActor
final class MemoPlayer: ObservableObject {
    @Published var playing = false
    @Published var progress: Double = 0
    @Published var durationLabel = "0:00"
    private var audio: AVAudioPlayer?
    private var tick: Timer?
    private var totalSeconds: Double = 0
    private var localData: Data?
    private var fetchTask: Task<Void, Never>?

    /// Resolve the memo length as soon as the bubble appears (not only on play).
    /// Remote `AVURLAsset` often returns no duration for m4a until the whole file is
    /// buffered (moov at the end), so download then probe with `AVAudioPlayer`.
    func loadDuration(url: URL, headers: [String: String]) {
        guard totalSeconds <= 0, localData == nil else { return }
        fetchTask?.cancel()
        fetchTask = Task { _ = await ensureLocal(url: url, headers: headers) }
    }

    func toggle(url: URL, headers: [String: String]) {
        if let audio {
            if audio.isPlaying {
                audio.pause()
                playing = false
                stopTick()
                if totalSeconds > 0 { durationLabel = Self.format(totalSeconds) }
            } else {
                Self.activatePlayback()
                audio.play()
                playing = true
                startTick()
            }
            return
        }
        playing = true
        fetchTask?.cancel()
        fetchTask = Task {
            guard await ensureLocal(url: url, headers: headers) else {
                playing = false
                return
            }
            guard !Task.isCancelled else {
                playing = false
                return
            }
            startPlaying()
        }
    }

    /// Download the blob (Bearer / `?token=`) so duration and playback do not
    /// depend on `AVPlayer` streaming authenticated URLs.
    private func ensureLocal(url: URL, headers: [String: String]) async -> Bool {
        if localData != nil, totalSeconds > 0 { return true }
        do {
            var req = URLRequest(url: url)
            for (k, v) in headers { req.setValue(v, forHTTPHeaderField: k) }
            let (data, response) = try await URLSession.shared.data(for: req)
            guard !Task.isCancelled else { return false }
            guard let http = response as? HTTPURLResponse, (200..<400).contains(http.statusCode), !data.isEmpty else {
                return false
            }
            let probe = try AVAudioPlayer(data: data)
            let d = probe.duration
            guard d.isFinite, d > 0 else { return false }
            guard !Task.isCancelled else { return false }
            localData = data
            totalSeconds = d
            if !playing { durationLabel = Self.format(d) }
            return true
        } catch {
            return false
        }
    }

    private func startPlaying() {
        guard let localData else { playing = false; return }
        Self.activatePlayback()
        do {
            let p = try AVAudioPlayer(data: localData)
            p.prepareToPlay()
            audio = p
            totalSeconds = p.duration
            guard p.play() else { playing = false; return }
            playing = true
            startTick()
        } catch {
            playing = false
        }
    }

    private func startTick() {
        stopTick()
        tick = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.updateProgress() }
        }
    }

    private func stopTick() {
        tick?.invalidate()
        tick = nil
    }

    private func updateProgress() {
        guard let audio else { return }
        let d = max(audio.duration, totalSeconds)
        guard d > 0 else { return }
        totalSeconds = d
        progress = min(1, audio.currentTime / d)
        if audio.isPlaying {
            let remaining = Int(max(0, d - audio.currentTime))
            durationLabel = String(format: "%d:%02d", remaining / 60, remaining % 60)
            return
        }
        // Natural end (user pause sets `playing = false` before stopping the tick).
        if playing {
            playing = false
            audio.currentTime = 0
            progress = 0
            durationLabel = Self.format(d)
            stopTick()
        }
    }

    /// Switch off the recorder's play-and-record session so the memo is audible.
    private static func activatePlayback() {
        let session = AVAudioSession.sharedInstance()
        try? session.setCategory(.playback, mode: .spokenAudio, options: [.duckOthers])
        try? session.setActive(true)
    }

    private static func format(_ seconds: Double) -> String {
        let s = Int(seconds.rounded())
        return String(format: "%d:%02d", s / 60, s % 60)
    }
}

/// Streaming fade (the "veil"): each appended chunk fades in by alpha only.
struct StreamingText: View {
    let text: String
    let font: Font
    let color: Color

    @State private var chunks: [(start: Int, end: Int, at: Date)] = []
    @State private var seen = 0
    @State private var lastArrival: Date?
    @State private var emaGapMs: Double = 120

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30.0, paused: chunks.isEmpty)) { context in
            render(now: context.date)
        }
        .onAppear { observe(text) }
        .onChange(of: text) { _, new in observe(new) }
    }

    private var fadeSeconds: Double { min(max(emaGapMs * 3, 120), 400) / 1000 }

    private func observe(_ new: String) {
        let len = new.utf8.count
        let now = Date()
        if len <= seen {
            seen = len
            chunks.removeAll { $0.start >= len }
            return
        }
        if let last = lastArrival {
            let gap = now.timeIntervalSince(last) * 1000
            emaGapMs = emaGapMs * 0.7 + gap * 0.3
        }
        lastArrival = now
        chunks.append((start: seen, end: len, at: now))
        seen = len
        let fade = fadeSeconds
        chunks.removeAll { now.timeIntervalSince($0.at) >= fade }
    }

    private func render(now: Date) -> Text {
        let fade = fadeSeconds
        let bytes = Array(text.utf8)
        var pieces = Text("")
        var cursor = 0
        let live = chunks.filter { now.timeIntervalSince($0.at) < fade && $0.start < bytes.count }
        for chunk in live.sorted(by: { $0.start < $1.start }) {
            let start = max(chunk.start, cursor)
            let end = min(chunk.end, bytes.count)
            if start > cursor {
                pieces = pieces + Text(string(bytes, cursor, start)).foregroundColor(color)
            }
            if end > start {
                let p = now.timeIntervalSince(chunk.at) / fade
                let alpha = 1 - pow(1 - p, 1.6)
                pieces = pieces + Text(string(bytes, start, end)).foregroundColor(color.opacity(alpha))
            }
            cursor = max(cursor, end)
        }
        if cursor < bytes.count {
            pieces = pieces + Text(string(bytes, cursor, bytes.count)).foregroundColor(color)
        }
        return pieces.font(font)
    }

    private func string(_ bytes: [UInt8], _ from: Int, _ to: Int) -> String {
        String(decoding: bytes[from..<to], as: UTF8.self)
    }
}
