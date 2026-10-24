// The message stream, Grok Bot style: assistant bubbles (#202020) left, user
// bubbles (#5C5C5C) right, centred muted system lines between them, file and
// voice-memo cards, one "<Project> is working" line while a turn runs.
// Prose only: tool calls, status noise, usage and JSON never render.

import ArbosFFI
import AVFoundation
import ImageIO
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
    /// The message replied to (`replyTo` on the row); empty on a forward header.
    var messageId: Id
    var text: String
    /// Set on a forwarded message: the header reads "Forwarded from <name>" with the
    /// forward glyph instead of a reply quote, and does not scroll anywhere.
    var forwardedFrom: String? = nil

    /// The header for a forwarded message (`forwardedFrom` on the row).
    static func forwarded(from name: String) -> Quote {
        Quote(messageId: "", text: name, forwardedFrom: name)
    }

    /// The header shows this many characters of the original before "…".
    static let maxChars = 64

    /// The quote for a message that replies to `replyTo`; an original that is not loaded
    /// (older than the snapshot) still gets a header so the reference is never lost.
    static func forReply(_ replyTo: String, in messages: [Message]) -> Quote {
        forReply(replyTo, in: Transcript.index(messages))
    }

    /// Same, over a prebuilt id → message index (one lookup per reply, not a scan).
    static func forReply(_ replyTo: String, in byId: [Id: Message]) -> Quote {
        let text = byId[replyTo].map(Self.text) ?? "Earlier message"
        return Quote(messageId: replyTo, text: text)
    }

    /// First words of a message: the whole message on one line (newlines and markdown
    /// flattened), cut on a word boundary; an attachment-only message names its file.
    static func text(of message: Message) -> String {
        line(of: message, maxChars: maxChars)
    }

    /// A whole message on one line, for quote headers and the composer's reply preview:
    /// markdown blocks (headings, bullets, fences, tables) and inline markers (`**`, `` ` ``,
    /// links) dropped, every newline a space, whitespace collapsed. Attachments are not part
    /// of it — callers name the file when there is no prose.
    static func oneLine(_ text: String) -> String {
        var words: [Substring] = []
        for block in Markdown.blocks(text) {
            let pieces: [String]
            switch block {
            case .paragraph(let t), .heading(_, let t): pieces = [t]
            case .bullets(let items), .numbered(let items): pieces = items
            case .code(_, let code): pieces = [code]
            case .table(let rows): pieces = rows.map { $0.replacingOccurrences(of: "|", with: " ") }
            }
            for piece in pieces {
                let plain = (try? AttributedString(markdown: piece, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
                    .map { String($0.characters) } ?? piece
                words.append(contentsOf: plain.split(whereSeparator: \.isWhitespace))
            }
        }
        return words.joined(separator: " ")
    }

    /// `line` cut to `maxChars` on a word boundary with an ellipsis; shorter lines come back
    /// as they are.
    static func firstWords(_ line: String, maxChars: Int) -> String {
        guard line.count > maxChars else { return line }
        var cut = String(line.prefix(maxChars))
        if let space = cut.lastIndex(of: " "), cut.distance(from: cut.startIndex, to: space) > maxChars / 2 {
            cut = String(cut[..<space])
        }
        while let last = cut.last, ",;:.".contains(last) { cut.removeLast() }
        return cut + "…"
    }

    /// The message flattened to one line (`oneLine`) and cut to `maxChars`; an attachment-only
    /// message names its file, an empty one says "Message".
    static func line(of message: Message, maxChars: Int) -> String {
        var line = oneLine(message.plainText)
        if line.isEmpty {
            for part in message.parts {
                if case .attachment(_, let a) = part { line = a.name; break }
            }
        }
        if line.isEmpty { return "Message" }
        return firstWords(line, maxChars: maxChars)
    }
}

enum RowKind: Equatable {
    case user(text: String, pending: Bool, quote: Quote?)
    /// Another person's message in a people chat: left bubble with avatar; the name above
    /// only when the sender changed (`named`, Grok Bot's group style) and the chat names its
    /// senders at all (group / channel, `rows(multiParty:)`); the name sits inside the bubble
    /// (Jacob, 2026-10-23). No time inside any bubble: the time is the transcript's
    /// drag-to-reveal only (Jacob, 2026-10-24, docs/mac-design-rules.md).
    case other(text: String, handle: String, avatar: String?, named: Bool, quote: Quote?)
    case assistant(blocks: [Block], live: Bool, caption: String?, quote: Quote? = nil)
    /// Shared agent in a people chat: same shape as `.other` (avatar + bubble, name on each
    /// reply — consecutive answers from the same agent must not merge, #1447). No
    /// "answering @x · owned by @y" meta — ownership lives in the profile pane
    /// (Jacob, 2026-10-22). `agentId` opens that pane from the avatar / name.
    /// `quote` is the user message this answers (`inReplyTo`).
    case member(blocks: [Block], live: Bool, handle: String, name: String?, avatar: String?, named: Bool, agentId: String?, quote: Quote?)
    /// Centred muted line: hand-offs ("Messaged ◉ Affine"), stops, dates.
    case system(icon: String?, text: String, avatarProject: Id?)
    /// Grok Bot question card: bold question, × top-right, lettered answer rows.
    /// `questionId` is nil for a legacy `waiting_user` status line (no `question` part).
    /// `lane`: in a people chat the card sits after the sender's avatar lane, like the bubble it
    /// belongs to (#1440 follow-up: the card must share the bubble's left edge and cap width).
    case question(questionId: String?, text: String, options: [ChoiceOption], multi: Bool, picked: [String], open: Bool, lane: Bool)
    /// A file card; tap opens the media viewer on it (Open → Quick Look).
    /// `caption` is an optional description bar under a video when the sender typed
    /// one with the attachment — not a separate text bubble.
    case file(item: MediaItem, subtitle: String, caption: String?)
    case voiceMemo(name: String, projectId: Id, ref: String?, fromUser: Bool)
    /// A picture; tap opens the media viewer at its place in the chat's album.
    /// `caption` is the description bar under the picture when the send carried text.
    case image(item: MediaItem, fromUser: Bool, caption: String?)
    /// "Forwarded from <name>" under a forwarded attachment (media-viewer-2026-10-23).
    case forwardHint(text: String, fromUser: Bool)
    case error(code: String, message: String)
}

struct Row: Identifiable, Equatable {
    var id: String
    var messageId: Id
    var role: Role
    var kind: RowKind
    /// Emoji reactions shown under this bubble (stamped on the last prose/media row of a message).
    var reactions: [MessageReaction] = []
    /// How many messages reply to this one via `replyTo` (swipe / long-press Reply); shown top-right when > 0.
    var replyCount: Int = 0
    /// The message's `createdAt` (ms), so the swipe-revealed time needs no lookup.
    var createdAt: Int64 = 0
    /// Space above this row (8 pt within a run, 24 pt at a sender / turn change,
    /// 20 pt around system lines), fixed at build time so a row's layout does not
    /// depend on its position in the visible window.
    var gapBefore: CGFloat = 0
}

enum Transcript {
    /// id → message, built once per rebuild; replies and the chat's row lookups use it
    /// instead of scanning the transcript per row.
    static func index(_ messages: [Message]) -> [Id: Message] {
        var byId: [Id: Message] = [:]
        byId.reserveCapacity(messages.count)
        for m in messages where byId[m.id] == nil { byId[m.id] = m }
        return byId
    }

    /// `me` = my `user:<id>` in a people chat; nil in project chats (every user row is mine).
    /// Membership lines ("Jacob added Home, hjkb and Doug") are relay-written system
    /// messages in the doc (#1349); nothing is synthesised here.
    /// `agentNames`: agent project id → display name from the chat's member list, the
    /// fallback when a reply row carries no `senderName` (older boxes).
    static func rows(_ messages: [Message], projectId: Id, subagentNames: [String: String] = [:], me: String? = nil, people: Bool = false, agentNames: [String: String] = [:], multiParty: Bool = false) -> [Row] {
        rows(messages, byId: index(messages), projectId: projectId, subagentNames: subagentNames, me: me, people: people, agentNames: agentNames, multiParty: multiParty)
    }

    /// Same, with the caller's `index(messages)` (the chat keeps it for its own lookups).
    /// `multiParty`: the chat names its senders — a group or a server channel (people and
    /// agents alike); a 1:1 chat (me ↔ person, me ↔ agent) labels nobody (Jacob, 2026-10-23).
    static func rows(_ messages: [Message], byId: [Id: Message], projectId: Id, subagentNames: [String: String] = [:], me: String? = nil, people: Bool = false, agentNames: [String: String] = [:], multiParty: Bool = false) -> [Row] {
        var rows: [Row] = []
        rows.reserveCapacity(messages.count + messages.count / 8)
        var lastAt: Int64 = 0
        // Who spoke last: a name sits above a bubble only when the sender changed or a date
        // line broke the run (Grok Bot's group style).
        var prevSender: String? = nil
        /// Hand-off rows by id (name, messages exchanged), merged into one line when consecutive.
        var handoffs: [String: (String, Int)] = [:]
        let replyCounts = Self.replyCounts(in: messages)
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
            // People-chat agent replies always name the sender (#1447): two answers in a
            // row from the same agent must not look like one run.
            let agentInPeople = people && (message.role == .assistant || message.role == .project)
            let named = multiParty && (dateBreak || prevSender != message.sender || agentInPeople)
            switch message.role {
            case .user:
                var hasImage = false
                var hasAttachment = false
                let mine = !people || me == nil || message.sender == me || message.sender == "user"
                // Collect picture / video attachments first so a typed description can sit
                // as a caption bar on the last one instead of a second bubble.
                var mediaParts: [(id: String, a: Attachment)] = []
                for part in message.parts {
                    if case .attachment(let id, let a) = part {
                        if a.isImage { hasImage = true }
                        hasAttachment = true
                        mediaParts.append((id, a))
                    }
                }
                let text = message.plainText
                // A forwarded attachment's "Forwarded from …" line is a hint under
                // the card, not a bubble of its own.
                let forwarded = hasAttachment ? MediaAlbum.forwardedFrom(text) : nil
                // Only pictures and videos take a description bar; a PDF + note
                // still uses a normal text bubble.
                let lastCaptionable = mediaParts.lastIndex { Self.isCaptionableMedia($0.a) }
                let mediaCaption: String? = {
                    guard forwarded == nil, lastCaptionable != nil, !text.isEmpty,
                          !Self.isGenericMediaLabel(text) else { return nil }
                    return text
                }()
                for (i, part) in mediaParts.enumerated() {
                    let cap = (i == lastCaptionable) ? mediaCaption : nil
                    // Another person's picture / file sits left like their bubble.
                    chunk.append(attachmentRow(id: "\(message.id)#\(part.id)", message: message, a: part.a,
                                               projectId: projectId, fromUser: mine, agentNames: agentNames,
                                               caption: cap))
                }
                if let forwarded {
                    chunk.append(Row(id: "\(message.id)#forwarded", messageId: message.id, role: mine ? .user : .project,
                                     kind: .forwardHint(text: forwarded, fromUser: mine)))
                }
                // Generic labels ("Image", "Photo", …) are not prose — the attachment
                // row already carries the media. Showing them as a bubble is noise.
                // A real description on a picture/video is the caption bar on that row.
                if !text.isEmpty, forwarded == nil, mediaCaption == nil, !(hasImage && Self.isGenericMediaLabel(text)) {
                    // A forward carries its "Forwarded from" header where a reply carries its quote.
                    let quote: Quote?
                    if let by = message.forwardedFrom?.trimmingCharacters(in: .whitespacesAndNewlines), !by.isEmpty {
                        quote = .forwarded(from: by)
                    } else {
                        quote = message.replyTo.flatMap { $0.isEmpty ? nil : Quote.forReply($0, in: byId) }
                    }
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
                // Admin deletion audits stay out of the chat (GroupActivity screen only).
                let text = message.plainText.trimmingCharacters(in: .whitespacesAndNewlines)
                if !text.isEmpty, !GroupActivity.isAuditLine(text) {
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
                // Quote the message this answers: `replyTo` is the platform reply the box sets by
                // the #1521 rule (always in groups / channels / people chats and for a row another
                // agent sent; in a 1:1 only when answering an earlier message than the latest).
                // Member bubbles from older boxes fall back to `inReplyTo`; a plain 1:1 answer to
                // the latest message carries no quote.
                let platformQuote = message.replyTo.flatMap { $0.isEmpty ? nil : Quote.forReply($0, in: byId) }
                let replyQuote = platformQuote ?? message.inReplyTo.flatMap { $0.isEmpty ? nil : Quote.forReply($0, in: byId) }
                // Error parts wait until the prose is known: under a real answer they stay the
                // chip with Retry; on an answer with no words they become the words.
                var errors: [(id: Id, code: String, text: String)] = []
                let rowsBeforeParts = chunk.count
                for (pix, part) in message.parts.enumerated() {
                    switch part {
                    case .text(let id, let text):
                        // Polish on display (Mac parity): glued "findingsDon't" / "have.Checking"
                        // and a trailing tool-narration sentence still read as chat.
                        let blocks = Self.proseBlocks(text, people: people)
                        if blocks.isEmpty { continue }
                        let live = streaming && pix == lastTextPart
                        if people, let h = message.senderHandle {
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                             kind: .member(blocks: blocks, live: live, handle: h, name: message.senderName,
                                                           avatar: message.senderAvatar, named: named && pix == firstTextPart,
                                                           agentId: agentId,
                                                           quote: pix == firstTextPart ? replyQuote : nil)))
                        } else if message.role == .project, let from = Self.projectSender(message) {
                            // Another agent wrote into this chat (agent-to-agent messaging):
                            // the group member bubble — its avatar, its colour, its name above.
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                             kind: .member(blocks: blocks, live: live, handle: from.handle, name: message.senderName,
                                                           avatar: nil, named: pix == firstTextPart, agentId: from.projectId,
                                                           quote: nil)))
                        } else {
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                             kind: .assistant(blocks: blocks, live: live, caption: nil,
                                                              quote: pix == firstTextPart ? platformQuote : nil)))
                        }
                    case .status(let id, .askUser, let text):
                        // agentd writes a `question` part and this status line; the part wins.
                        if message.questions.isEmpty {
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                             kind: .question(questionId: nil, text: text, options: [], multi: false, picked: [], open: true,
                                                             lane: people && message.senderHandle != nil)))
                        }
                    case .question(let id, let q):
                        chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                         kind: .question(questionId: q.questionId, text: q.question, options: q.options,
                                                         multi: q.multi, picked: q.answer, open: q.open,
                                                         lane: people && message.senderHandle != nil)))
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
                        chunk.append(attachmentRow(id: "\(message.id)#\(id)", message: message, a: a, projectId: projectId, fromUser: false, agentNames: agentNames))
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
                // A hand-off row (a subagent brief, an agent-to-agent send) is its own line
                // ("Messaged ◉ X"); it never gets the stock line, even with no parts (#1466).
                let handoffRow = handoffName != nil
                if showsNothing, finished, !streaming, !handoffRow, failed || onlyEmpty {
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
                                         kind: .member(blocks: blocks, live: false, handle: h, name: message.senderName,
                                                       avatar: message.senderAvatar, named: named, agentId: agentId,
                                                       quote: replyQuote)))
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
            // Agent-to-agent messaging: the reply went back to the agent that wrote in.
            if !streaming, let to = message.agentReplyTo?.trimmingCharacters(in: .whitespaces), !to.isEmpty,
               message.role == .assistant {
                chunk.append(Row(id: "\(message.id)#replied", messageId: message.id, role: .system,
                                 kind: .system(icon: nil, text: "Replied to \u{FFFC}\(to)", avatarProject: nil)))
            }
            guard !chunk.isEmpty else { continue }
            for i in chunk.indices { chunk[i].createdAt = message.createdAt }
            if !message.reactions.isEmpty,
               let ix = chunk.lastIndex(where: {
                   switch $0.kind {
                   case .user, .other, .member, .assistant, .image, .file, .voiceMemo: return true
                   default: return false
                   }
               }) {
                chunk[ix].reactions = message.reactions
            }
            // Reply count on the first prose/media row so the badge sits on the message's top bubble.
            if let count = replyCounts[message.id], count > 0,
               let ix = chunk.firstIndex(where: {
                   switch $0.kind {
                   case .user, .other, .member, .assistant, .image, .file, .voiceMemo: return true
                   default: return false
                   }
               }) {
                chunk[ix].replyCount = count
            }
            // Date separators when the gap is over an hour (Grok: "Today 8:00 AM").
            if dateBreak {
                rows.append(Row(id: "\(message.id)#date", messageId: message.id, role: .system,
                                kind: .system(icon: nil, text: dateLabel(message.createdAt), avatarProject: nil),
                                createdAt: message.createdAt))
            }
            if message.createdAt > 0 { lastAt = message.createdAt }
            rows.append(contentsOf: chunk)
            prevSender = message.sender
        }
        return Self.withGaps(Self.mergeHandoffs(rows, handoffs))
    }

    /// Spacing between neighbours (measured on the Grok Bot references).
    static func gap(from prev: Row, to cur: Row) -> CGFloat {
        if case .system = prev.kind { return 20 }
        if case .system = cur.kind { return 20 }
        if prev.role != cur.role { return 24 }
        // Same role: still open the turn gap when an agent starts a new message.
        if prev.messageId != cur.messageId && (cur.role == .assistant || cur.role == .project) {
            return 24
        }
        return 8
    }

    private static func withGaps(_ rows: [Row]) -> [Row] {
        var out = rows
        for i in out.indices.dropFirst() { out[i].gapBefore = gap(from: out[i - 1], to: out[i]) }
        return out
    }

    /// Parent message id for an explicit reply (`replyTo` from swipe / long-press Reply).
    /// Agent turn links (`inReplyTo`) are not comment-thread parents — counting them would
    /// put a badge on every answered user message in a 1:1 agent chat.
    static func replyParentId(of message: Message) -> Id? {
        if let p = message.replyTo, !p.isEmpty { return p }
        return nil
    }

    /// How many messages point at each parent via `replyTo`.
    static func replyCounts(in messages: [Message]) -> [Id: Int] {
        var counts: [Id: Int] = [:]
        for message in messages {
            guard let parent = replyParentId(of: message) else { continue }
            counts[parent, default: 0] += 1
        }
        return counts
    }

    /// Direct and nested `replyTo` replies to `rootId`, in transcript order (root itself excluded).
    static func threadReplies(rootId: Id, in messages: [Message]) -> [Message] {
        var parentOf: [Id: Id] = [:]
        parentOf.reserveCapacity(messages.count)
        for message in messages {
            if let parent = replyParentId(of: message) { parentOf[message.id] = parent }
        }
        func inThread(_ id: Id) -> Bool {
            var cur: Id? = id
            var guardCount = 0
            while let c = cur, guardCount < 64 {
                if c == rootId { return true }
                cur = parentOf[c]
                guardCount += 1
            }
            return false
        }
        return messages.filter { $0.id != rootId && inThread($0.id) }
    }

    /// Blocks for one text part — polished, split, JSON dropped. Cached by text: a rebuild
    /// runs for every streaming delta, and re-parsing every finished answer in a long group
    /// on each one kept the main thread busy. Only the streaming part misses the cache.
    static func proseBlocks(_ text: String, people: Bool) -> [Block] {
        let key = (people ? "p\u{1}" : "a\u{1}") + text
        if let hit = blockCache.object(forKey: key as NSString) { return hit.blocks }
        let raw = people ? Self.stripAskedPrefix(text) : text
        let body = Markdown.polishAssistantProse(raw)
        let blocks = Markdown.blocks(body).filter { !Markdown.isJSON($0) }
        blockCache.setObject(BlockList(blocks), forKey: key as NSString)
        return blocks
    }

    private final class BlockList {
        let blocks: [Block]
        init(_ blocks: [Block]) { self.blocks = blocks }
    }

    /// Bounded (NSCache evicts under memory pressure and past `countLimit`).
    private static let blockCache: NSCache<NSString, BlockList> = {
        let cache = NSCache<NSString, BlockList>()
        cache.countLimit = 6_000
        return cache
    }()

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

    /// `sender: project:<id>` plus the row's `senderHandle` (or a slug of `senderName`): the
    /// handle the member bubble is drawn with and the agent id its tap opens.
    static func projectSender(_ message: Message) -> (handle: String, projectId: String)? {
        guard message.sender.hasPrefix("project:") else { return nil }
        let pid = String(message.sender.dropFirst("project:".count))
        let handle = message.senderHandle ?? message.senderName.map { mentionSlug($0) } ?? pid
        return (handle, pid)
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

    private static func attachmentRow(id: String, message: Message, a: Attachment, projectId: Id, fromUser: Bool,
                                      agentNames: [String: String], caption: String? = nil) -> Row {
        guard let kind = MediaAlbum.kind(of: a) else {
            return Row(id: id, messageId: message.id, role: fromUser ? .user : .assistant,
                       kind: .voiceMemo(name: a.name, projectId: projectId, ref: a.blobRef, fromUser: fromUser))
        }
        let item = MediaAlbum.item(id: id, message: message, attachment: a, kind: kind, slot: projectId,
                                   projectName: { agentNames[$0] })
        if kind == .image {
            return Row(id: id, messageId: message.id, role: fromUser ? .user : .assistant,
                       kind: .image(item: item, fromUser: fromUser, caption: caption))
        }
        // Video shows as a file card in the bubble (poster + Play live in the viewer).
        // A typed description rides the card as a caption bar (pictures/videos only).
        return Row(id: id, messageId: message.id, role: fromUser ? .user : .assistant,
                   kind: .file(item: item, subtitle: subtitle(for: a),
                               caption: kind == .video ? caption : nil))
    }

    /// Picture or video — the kinds that take a description bar with the send.
    private static func isCaptionableMedia(_ a: Attachment) -> Bool {
        switch MediaAlbum.kind(of: a) {
        case .image, .video: return true
        case .file, .none: return false
        }
    }

    /// UTType-style labels some clients leave as the only text on a media-only send.
    private static func isGenericMediaLabel(_ text: String) -> Bool {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return t == "image" || t == "photo" || t == "video" || t == "attachment" || t == "(attachment)"
    }

    /// Grok's doc card subline: "PDF · 1.8 MB" (the kind alone when the size is unknown).
    private static func subtitle(for a: Attachment) -> String {
        let size = a.sizeLabel
        return size.isEmpty ? a.kindLabel : "\(a.kindLabel) · \(size)"
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
        // Parsed once per text and palette: the markdown parse and the link detector
        // ran on every bubble body, and a long group re-evaluates many bodies at once
        // (arbonites-chat-lag). NSCache: bounded, dropped under memory pressure.
        let key = "\(text)\u{1}\(color)\u{1}\(link)\u{1}\(codeChip)" as NSString
        if let hit = inlineCache.object(forKey: key) { return hit.value }
        let attr = inlineUncached(text, color: color, link: link, codeChip: codeChip)
        inlineCache.setObject(AttributedBox(attr), forKey: key)
        return attr
    }

    private final class AttributedBox {
        let value: AttributedString
        init(_ value: AttributedString) { self.value = value }
    }

    private static let inlineCache: NSCache<NSString, AttributedBox> = {
        let cache = NSCache<NSString, AttributedBox>()
        cache.countLimit = 4_000
        return cache
    }()

    private static func inlineUncached(_ text: String, color: Color, link: Color, codeChip: Color) -> AttributedString {
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

struct RowView: View, Equatable {
    /// Held without `@EnvironmentObject` / `@ObservedObject` so presence, typing
    /// and the other ~60 `@Published` AppModel fields do not invalidate every
    /// bubble. ChatView still observes the model and rebuilds rows when the
    /// transcript changes; actions call through this reference.
    let model: AppModel
    /// Plain inputs, not `@Environment`: an environment read made every bubble's body
    /// re-run on each AppModel publish even when the value was equal (SwiftUI marks the
    /// environment dependency changed per write) — the open-time freeze on long groups
    /// (iphone-open-scroll-block, 2026-10-24). The row list reads them once.
    let theme: Theme
    let composerInset: CGFloat
    let row: Row
    let chatId: Id
    let width: CGFloat
    /// How far the transcript has been dragged left to reveal iMessage-style times (0…`timeRevealWidth`).
    var timeReveal: CGFloat = 0
    /// A tap on a reply bubble's quote header: the chat scrolls to this message id.
    var onQuoteTap: ((Id) -> Void)? = nil
    /// Briefly tinted after a quote tap landed on this row's message.
    var highlighted: Bool = false
    /// Swipe right on this bubble → reply (same as the long-press Reply item).
    /// Nil on system / channel / non-message rows.
    var onReply: (() -> Void)? = nil
    /// ChatView sets this while a reply swipe is locked so swipe-right-to-back does not fire.
    var replySwipeActive: Binding<Bool> = .constant(false)
    /// Shared mention table from ChatView (built once per directory sync).
    let mentions: MentionTable

    /// Trailing column width for swipe-revealed times (fits "12:59 PM").
    static let timeRevealWidth: CGFloat = 56
    /// How far a right swipe pulls the bubble before reply arms (Telegram-style).
    static let replySwipeWidth: CGFloat = 56

    @State private var replyDrag: CGFloat = 0
    @State private var replyLocked = false

    /// Skip re-drawing when the parent re-evaluates for an unrelated reason
    /// (composer keystrokes, presence ticks). Closures / the swipe binding are
    /// not compared — they stay valid for the open chat.
    static func == (lhs: RowView, rhs: RowView) -> Bool {
        let same = lhs.row == rhs.row
            && lhs.chatId == rhs.chatId
            && lhs.width == rhs.width
            && lhs.timeReveal == rhs.timeReveal
            && lhs.highlighted == rhs.highlighted
            && lhs.mentions.stamp == rhs.mentions.stamp
            && lhs.composerInset == rhs.composerInset
            && lhs.theme == rhs.theme
        #if DEBUG
        PerfCounters.bump(same ? "rowEqTrue" : "rowEqFalse")
        #endif
        return same
    }

    private var maxBubble: CGFloat { max(200, width - Theme.bubbleInset) }

    private var reportsGreetingFrame: Bool {
        model.firstLogin != nil && model.messages(of: chatId).first(where: { $0.greeting })?.id == row.messageId
    }

    /// Bubbles get a swipe time; centred system / card / error rows do not.
    private var showsSwipeTime: Bool {
        switch row.kind {
        case .system, .question, .error, .forwardHint:
            return false
        case .user, .other, .member, .assistant, .file, .voiceMemo, .image:
            return true
        }
    }

    private var swipeTimeLabel: String? {
        guard showsSwipeTime, row.createdAt > 0 else { return nil }
        return Date(timeIntervalSince1970: Double(row.createdAt) / 1000)
            .formatted(date: .omitted, time: .shortened)
    }

    var body: some View {
        #if DEBUG
        let _ = PerfCounters.bump("rowBody")
        let _ = DebugHooks.perfChanges ? Self._printChanges() : ()
        #endif
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
        // iMessage: drag left → times; Telegram: drag right → reply. Mutually exclusive.
        .offset(x: replyDrag - timeReveal)
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
        .overlay(alignment: .leading) {
            if onReply != nil, replyDrag > 0.5 {
                Image(systemName: "arrowshape.turn.up.left")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(theme.accent)
                    .frame(width: Self.replySwipeWidth, alignment: .leading)
                    .offset(x: replyDrag - Self.replySwipeWidth)
                    .opacity(Double(min(1, replyDrag / (Self.replySwipeWidth * 0.55))))
                    .allowsHitTesting(false)
                    .accessibilityHidden(replyDrag < 12)
            }
        }
        .simultaneousGesture(replySwipeGesture)
    }

    /// Drag right on a message to reply. Locks only after a clear horizontal-right
    /// pan so vertical scroll and swipe-left time reveal stay free.
    private var replySwipeGesture: some Gesture {
        DragGesture(minimumDistance: 12, coordinateSpace: .local)
            .onChanged { value in
                guard onReply != nil else { return }
                let dx = value.translation.width
                let dy = value.translation.height
                if !replyLocked {
                    guard abs(dx) + abs(dy) >= 12 else { return }
                    // Vertical scroll or swipe-left time peek: do not steal.
                    guard dx > 0, abs(dx) > abs(dy) * 1.15 else { return }
                    replyLocked = true
                    replySwipeActive.wrappedValue = true
                }
                let raw = max(0, dx)
                let w = Self.replySwipeWidth
                let reveal = raw <= w ? raw : w + min(24, (raw - w) * 0.2)
                var t = Transaction()
                t.disablesAnimations = true
                withTransaction(t) { replyDrag = reveal }
            }
            .onEnded { _ in
                let triggered = replyLocked && replyDrag >= Self.replySwipeWidth * 0.7
                replyLocked = false
                withAnimation(.spring(response: 0.34, dampingFraction: 0.86)) {
                    replyDrag = 0
                }
                if triggered {
                    Haptics.pick()
                    onReply?()
                }
                // Keep the back-swipe suppress flag until after ChatView's simultaneous
                // swipe-right-to-Home onEnded has run (same drag).
                DispatchQueue.main.async {
                    replySwipeActive.wrappedValue = false
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
                    // No `.textSelection(.enabled)`: it steals pans from the chat
                    // ScrollView so a drag on / onto a bubble sticks
                    // ([feedback app-01M5Q1N6C1HAKHC6P2XPEWMRSS]). Copy is on long-press.
                    Markdown.inlineText(text, color: theme.userText, link: theme.accent, codeChip: theme.userCodeChip, mentions: mentions)
                        .font(theme.font(Theme.body))
                        .lineSpacing(Theme.bodyLineSpacing)
                        .tint(theme.accent)
                }
                .padding(.horizontal, Theme.bubbleHPad)
                .padding(.vertical, Theme.bubbleVPad)
                .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleUser))
                .overlay(alignment: .topTrailing) { replyCountBadge(userBubble: true) }
                .frame(maxWidth: maxBubble, alignment: .trailing)
                .opacity(pending ? 0.6 : 1)
            }
        case .other(let text, let handle, let avatar, let named, let quote):
            let openPerson = { model.openPersonProfile(handle: handle, avatarUrl: avatar, chatId: chatId) }
            HStack(alignment: .bottom, spacing: Theme.memberAvatarGap) {
                Button(action: openPerson) {
                    PersonAvatar(url: avatar, handle: handle, size: Theme.memberAvatar, name: model.personName(handle: handle), id: model.personId(handle: handle))
                }
                .buttonStyle(.plain)
                .padding(.leading, Theme.avatarColumnInset(composerInset: composerInset))
                .accessibilityLabel("@\(handle), profile")
                // Group / channel: the name inside the bubble, in the sender's colour,
                // then the reply quote, then the text. No time in the bubble (drag to reveal).
                VStack(alignment: .leading, spacing: 0) {
                    if named {
                        Button(action: openPerson) {
                            LiveSenderName(name: model.personName(handle: handle) ?? "@\(handle)",
                                           url: avatar, seed: model.personId(handle: handle) ?? handle, agent: false)
                        }
                        .buttonStyle(.plain)
                        .padding(.bottom, Self.nameToTextGap)
                        .accessibilityIdentifier("bubble-name")
                    }
                    if let quote {
                        quoteHeader(quote, color: theme.text)
                            .padding(.bottom, Self.quoteToTextGap)
                    }
                    Markdown.inlineText(text, color: theme.text, link: theme.accent, codeChip: theme.codeChip, mentions: mentions)
                        .font(theme.font(Theme.body))
                        .lineSpacing(Theme.bodyLineSpacing)
                        .tint(theme.accent)
                }
                .padding(.horizontal, Theme.bubbleHPad)
                .padding(.vertical, Theme.bubbleVPad)
                .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleAssistant))
                .overlay(alignment: .topTrailing) { replyCountBadge(userBubble: false) }
                .modifier(LiveSenderBar(url: avatar, seed: model.personId(handle: handle) ?? handle))
                .frame(maxWidth: maxBubble - Theme.memberLane(composerInset: composerInset), alignment: .leading)
                Spacer(minLength: 0)
            }
            .task(id: avatar) { SenderAccents.shared.load(url: avatar) }
        case .member(let blocks, let live, let handle, let name, let avatar, let named, let agentId, let quote):
            let openMember = {
                if let agentId { model.showProfile(.agent(projectId: agentId)) }
                else { model.openPersonProfile(handle: handle, avatarUrl: avatar, chatId: chatId) }
            }
            // One identity for the disc, the caption and the bar (#1592): the same resolver
            // the presence row asks, with the same row fields.
            let agent = model.agent(sender: agentId ?? "", handle: handle, name: name, chatId: chatId)
            HStack(alignment: .bottom, spacing: Theme.memberAvatarGap) {
                Button(action: openMember) { memberAvatar(agent) }
                    .padding(.leading, Theme.avatarColumnInset(composerInset: composerInset))
                    .buttonStyle(.plain)
                    .accessibilityLabel("@\(handle), profile")
                memberBubble(
                    agent,
                    content: VStack(alignment: .leading, spacing: 0) {
                        if named {
                            Button(action: openMember) {
                                memberName(agent)
                            }
                            .buttonStyle(.plain)
                            .padding(.bottom, Self.nameToTextGap)
                            .accessibilityIdentifier("bubble-name")
                        }
                        if let quote {
                            quoteHeader(quote, color: theme.text)
                                .padding(.bottom, Self.quoteToTextGap)
                        }
                        VStack(alignment: .leading, spacing: 8) {
                            ForEach(Array(blocks.enumerated()), id: \.offset) { i, block in
                                BlockView(block: block, live: live && i == blocks.count - 1, mentions: mentions)
                            }
                        }
                    }
                )
                .overlay(alignment: .topTrailing) { replyCountBadge(userBubble: false) }
                .frame(maxWidth: maxBubble - Theme.memberLane(composerInset: composerInset), alignment: .leading)
                Spacer(minLength: 0)
            }
        case .assistant(let blocks, let live, let caption, let quote):
            HStack {
                VStack(alignment: .leading, spacing: 8) {
                    if let caption {
                        Text(caption)
                            .font(theme.font(Theme.tiny, weight: .medium))
                            .foregroundStyle(theme.muted)
                    }
                    if let quote {
                        quoteHeader(quote, color: theme.text)
                    }
                    ForEach(Array(blocks.enumerated()), id: \.offset) { i, block in
                        BlockView(block: block, live: live && i == blocks.count - 1, mentions: mentions)
                    }
                }
                .padding(.horizontal, Theme.bubbleHPad)
                .padding(.vertical, Theme.bubbleVPad)
                .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleAssistant))
                .overlay(alignment: .topTrailing) { replyCountBadge(userBubble: false) }
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
        case .question(let questionId, let text, let options, let multi, let picked, let open, let lane):
            // The card is a transcript row like a bubble: the same left edge (after the avatar
            // lane in a people chat) and the same cap width as an incoming bubble.
            HStack(spacing: 0) {
                if lane { Color.clear.frame(width: Theme.memberLane(composerInset: composerInset), height: 1) }
                QuestionCard(rowId: row.id, chatId: chatId, questionId: questionId, title: text, options: options,
                             multi: multi, picked: picked, open: open)
                    .frame(maxWidth: lane ? maxBubble - Theme.memberLane(composerInset: composerInset) : maxBubble, alignment: .leading)
                Spacer(minLength: 0)
            }
        case .file(let item, let subtitle, let caption):
            HStack {
                if row.role == .user { Spacer(minLength: 0) }
                FileCard(item: item, subtitle: subtitle, chatId: chatId, fromUser: row.role == .user, caption: caption)
                    .overlay(alignment: .topTrailing) { replyCountBadge(userBubble: row.role == .user) }
                    .frame(maxWidth: maxBubble, alignment: row.role == .user ? .trailing : .leading)
                if row.role != .user { Spacer(minLength: 0) }
            }
        case .forwardHint(let text, let fromUser):
            HStack {
                if fromUser { Spacer(minLength: 0) }
                Text(text).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(1)
                    .padding(.horizontal, 6)
                if !fromUser { Spacer(minLength: 0) }
            }
        case .voiceMemo(let name, let projectId, let ref, let fromUser):
            HStack {
                if fromUser { Spacer(minLength: 0) }
                VoiceMemoBubble(name: name, projectId: projectId, blobRef: ref, fromUser: fromUser)
                    .overlay(alignment: .topTrailing) { replyCountBadge(userBubble: fromUser) }
                    .frame(maxWidth: maxBubble, alignment: fromUser ? .trailing : .leading)
                if !fromUser { Spacer(minLength: 0) }
            }
        case .image(let item, let fromUser, let caption):
            HStack {
                if fromUser { Spacer(minLength: 0) }
                ImageBubble(item: item, chatId: chatId, fromUser: fromUser, maxWidth: maxBubble, caption: caption)
                    .overlay(alignment: .topTrailing) { replyCountBadge(userBubble: fromUser) }
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

    /// Reply count in the bubble's top-right corner when this message has replies.
    @ViewBuilder
    private func replyCountBadge(userBubble: Bool) -> some View {
        if row.replyCount > 0 {
            Text("\(row.replyCount)")
                .font(theme.font(11, weight: .semibold))
                .foregroundStyle(userBubble ? theme.userText.opacity(0.7) : theme.muted)
                .padding(.top, 6)
                .padding(.trailing, 8)
                .accessibilityLabel("\(row.replyCount) \(row.replyCount == 1 ? "reply" : "replies")")
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
        case .other, .member: return Theme.memberLane(composerInset: composerInset)
        case .question(_, _, _, _, _, _, let lane): return lane ? Theme.memberLane(composerInset: composerInset) : 0
        default: return 0
        }
    }

    /// The disc beside a people-chat agent's bubble: the agent's picture through the shared
    /// identity (`AppModel.agent`) — its uploaded picture, its leaf, never initials seeded by
    /// the handle (#1592).
    private func memberAvatar(_ agent: Project) -> some View {
        AgentAvatar(project: agent, theme: theme, size: Theme.memberAvatar, still: true)
    }

    /// Jacob's Discord-style reference (2026-10-23): 2 pt from the name to what follows,
    /// 6 from the reply quote to the text (#1393).
    static let nameToTextGap: CGFloat = 2
    static let quoteToTextGap: CGFloat = 6

    /// A people-chat member's name inside its bubble, in the same colour as its bar
    /// (`memberBubble`), with the `agent` tag: the identity's display name, never the handle.
    @ViewBuilder
    private func memberName(_ agent: Project) -> some View {
        if LeafAvatarsFlag.shared.on {
            LiveSenderName(name: agent.name, url: nil, seed: agent.id, fixed: LeafArt.accent(agent, dark: theme.isDark), agent: true)
        } else {
            LiveSenderName(name: agent.name, url: nil, seed: agent.id, override: agent.effectiveAvatarColorIndex, agent: true)
        }
    }

    /// Mac parity: an agent's bubbles in a people chat carry its disc colour
    /// (the leaf's disc under D92, the D79 gradient midpoint otherwise). Accent
    /// observation stays in `LiveSenderBar` so one new colour does not rebuild
    /// every bubble in a group chat.
    @ViewBuilder
    private func memberBubble<Content: View>(_ agent: Project, content: Content) -> some View {
        if LeafAvatarsFlag.shared.on {
            content
                .padding(.horizontal, Theme.bubbleHPad)
                .padding(.vertical, Theme.bubbleVPad)
                .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleAssistant))
                .senderBar(LeafArt.accent(agent, dark: theme.isDark))
        } else {
            content
                .padding(.horizontal, Theme.bubbleHPad)
                .padding(.vertical, Theme.bubbleVPad)
                .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleAssistant))
                .modifier(LiveSenderBar(url: nil, seed: agent.id, override: agent.effectiveAvatarColorIndex))
        }
    }

    /// Grok Bot's quote header at the top of a reply bubble: speech-bubble glyph, then the
    /// original's first words in quotes on one line. Tap → the chat scrolls to the original.
    @ViewBuilder
    private func quoteHeader(_ quote: Quote, color: Color) -> some View {
        if let by = quote.forwardedFrom {
            // Telegram / iMessage: forward glyph + "Forwarded from <name>", one line, not tappable.
            HStack(spacing: 6) {
                Image(systemName: "arrowshape.turn.up.right")
                    .font(.system(size: 11, weight: .medium))
                Text(MediaAlbum.forwardedPrefix + by)
                    .font(theme.font(Theme.small, weight: .medium))
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            .foregroundStyle(color.opacity(0.72))
            .accessibilityLabel("Forwarded from \(by)")
        } else {
            replyQuoteHeader(quote, color: color)
        }
    }

    private func replyQuoteHeader(_ quote: Quote, color: Color) -> some View {
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
    /// Shared mention table from the row (built once per directory sync).
    let mentions: MentionTable

    var body: some View {
        #if DEBUG
        let _ = PerfCounters.bump("blockBody")
        #endif
        switch block {
        case .paragraph(let text):
            if live {
                StreamingText(text: text, font: .system(size: Theme.body), color: theme.text)
                    .lineSpacing(Theme.bodyLineSpacing)
            } else {
                Markdown.inlineText(text, color: theme.text, link: theme.accent, codeChip: theme.codeChip, mentions: mentions)
                    .font(theme.font(Theme.body))
                    .lineSpacing(Theme.bodyLineSpacing)
                    .tint(theme.accent)
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
                        // HStack otherwise gives the text a single-line width
                        // proposal and it truncates with "…"; grow vertically.
                        if live && i == items.count - 1 {
                            StreamingText(text: item, font: .system(size: Theme.body), color: theme.text)
                                .fixedSize(horizontal: false, vertical: true)
                        } else {
                            Markdown.inlineText(item, color: theme.text, link: theme.accent, codeChip: theme.codeChip, mentions: mentions)
                                .font(theme.font(Theme.body))
                                .tint(theme.accent)
                                .fixedSize(horizontal: false, vertical: true)
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

/// The sender's name inside a group / channel bubble (Jacob's Discord-style reference,
/// 2026-10-23): semibold in the sender's identity colour — the same colour as the bubble's
/// bar — with a small `agent` tag for agents. Observes `SenderAccents` for this one label only.
private struct LiveSenderName: View {
    @ObservedObject private var accents = SenderAccents.shared
    @Environment(\.theme) private var theme
    let name: String
    let url: String?
    let seed: String
    var override: Int? = nil
    var fixed: Color? = nil
    let agent: Bool

    var body: some View {
        let color = fixed ?? accents.color(url: url, seed: seed, override: override, dark: theme.isDark)
        HStack(spacing: 6) {
            Text(name)
                .font(theme.font(Theme.small, weight: .semibold))
                .foregroundStyle(color)
                .lineLimit(1)
            if agent {
                Text("agent")
                    .font(theme.font(10, weight: .semibold))
                    .foregroundStyle(color)
                    .padding(.horizontal, 5)
                    .frame(height: 16)
                    .background(RoundedRectangle(cornerRadius: 4, style: .continuous).fill(color.opacity(0.16)))
            }
        }
    }
}

/// Observes `SenderAccents` for one bubble only — a new colour must not rebuild
/// every `RowView` in a group chat (that made scrolling feel sticky).
private struct LiveSenderBar: ViewModifier {
    @ObservedObject private var accents = SenderAccents.shared
    @Environment(\.theme) private var theme
    let url: String?
    let seed: String
    var override: Int? = nil

    func body(content: Content) -> some View {
        content.senderBar(accents.color(url: url, seed: seed, override: override, dark: theme.isDark))
    }
}

/// Decoded people pictures for the launch, keyed by URL. A picture that is here
/// paints on the first frame of any row that shows it (`PersonAvatar` reads it
/// synchronously); `warm` fetches the New Message directory's pictures ahead of
/// the sheet, so its rows never flash initials and then the photo.
///
/// Not an `ObservableObject`: one finished download must not invalidate every
/// avatar on screen (group-chat scroll lag). Each `PersonAvatar` holds its own
/// `@State` and awaits `load`.
@MainActor
final class AvatarImageCache {
    static let shared = AvatarImageCache()

    private(set) var images: [String: UIImage] = [:]
    private var requested: Set<String> = []
    private var waiters: [String: [CheckedContinuation<UIImage?, Never>]] = [:]

    func image(for url: String?) -> UIImage? {
        url.flatMap { images[$0] }
    }

    /// Pictures are kept at the largest size a row draws (128 pt hero at 3x).
    nonisolated static let maxPixelSize = 384
    /// How many of a list's pictures `warm` fetches ahead (several screens of rows;
    /// the rest load as they scroll in).
    nonisolated static let warmLimit = 48

    /// Start fetching the first `warmLimit` URLs not seen this launch (through the shared URLCache).
    func warm(urls: [String?]) {
        for case let url? in urls.prefix(AvatarImageCache.warmLimit) {
            Task { _ = await load(url) }
        }
    }

    /// Fire-and-forget fetch (directory warm / legacy callers).
    func fetch(_ url: String) {
        Task { _ = await load(url) }
    }

    /// Shared download per URL; waiters resume together when the picture lands.
    func load(_ url: String) async -> UIImage? {
        if let img = images[url] { return img }
        if requested.contains(url) {
            return await withCheckedContinuation { cont in
                waiters[url, default: []].append(cont)
            }
        }
        guard url.contains("://"), let u = URL(string: url) else { return nil }
        requested.insert(url)
        let img: UIImage? = await Task.detached(priority: .userInitiated) {
            guard let (data, _) = try? await URLSession.shared.data(from: u) else { return nil }
            return AvatarImageCache.decode(data)
        }.value
        if let img {
            images[url] = img
        } else {
            requested.remove(url)
        }
        let pending = waiters.removeValue(forKey: url) ?? []
        for cont in pending { cont.resume(returning: img) }
        return img
    }

    /// Downsampled decode through ImageIO: a 460 px GitHub picture never sits in memory at full size.
    nonisolated static func decode(_ data: Data) -> UIImage? {
        guard let src = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        let opts: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixelSize,
        ]
        guard let cg = CGImageSourceCreateThumbnailAtIndex(src, 0, opts as CFDictionary) else { return nil }
        return UIImage(cgImage: cg)
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
    @State private var image: UIImage?

    /// Warm cache hits paint on the first frame (no initials flash); `@State`
    /// covers pictures that finish loading after appear — without observing the
    /// whole shared dictionary.
    private var displayed: UIImage? {
        image ?? AvatarImageCache.shared.image(for: url)
    }

    var body: some View {
        ZStack {
            if let displayed {
                Image(uiImage: displayed).resizable().scaledToFill()
            } else {
                initials
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .task(id: url) {
            guard let url else {
                image = nil
                return
            }
            if let cached = AvatarImageCache.shared.image(for: url) {
                image = cached
                return
            }
            image = await AvatarImageCache.shared.load(url)
        }
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

/// Inline picture (feedback chat-preview-cards-2026-10-23, Grok-black): the
/// picture is its own bubble — uniform margins trimmed, aspect-fit at the
/// bubble radius, no frame and no footer (Download / Forward live in the viewer
/// it opens). Loaded with the device bearer (same as voice memos).
/// When the send carried a description, a caption bar sits under the picture
/// in the same bubble so the note is not a second message.
struct ImageBubble: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let item: MediaItem
    let chatId: Id
    let fromUser: Bool
    let maxWidth: CGFloat
    var caption: String? = nil

    @State private var image: UIImage?
    @State private var failed = false
    @State private var viewer: MediaViewerState?

    /// Mac `picture_size`: fill the bubble width unless the picture is tall.
    static let maxHeight: CGFloat = 320
    static let minWidth: CGFloat = 120
    static let placeholder = CGSize(width: 240, height: 160)

    private var bubbleWidth: CGFloat { min(280, maxWidth) }
    private var fill: Color { fromUser ? theme.bubbleUser : theme.bubbleAssistant }
    private var captionText: String? {
        guard let caption else { return nil }
        let t = caption.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }

    private var size: CGSize {
        guard let image, image.size.width > 0, image.size.height > 0 else { return Self.placeholder }
        return Self.fit(image.size, maxWidth: bubbleWidth)
    }

    static func fit(_ natural: CGSize, maxWidth: CGFloat) -> CGSize {
        let scale = min(maxWidth / natural.width, maxHeight / natural.height)
        var w = min(natural.width * scale, maxWidth)
        var h = min(natural.height * scale, maxHeight)
        if w < minWidth {
            h = min(h * minWidth / w, maxHeight)
            w = minWidth
        }
        return CGSize(width: w.rounded(), height: h.rounded())
    }

    var body: some View {
        Button { viewer = MediaOpen.state(for: item, chatId: chatId, model: model) } label: {
            VStack(alignment: .leading, spacing: 0) {
                ZStack {
                    fill
                    if let image {
                        Image(uiImage: image)
                            .resizable()
                            .scaledToFit()
                    } else if failed || item.blobRef == nil {
                        Image(systemName: "photo")
                            .font(theme.font(28))
                            .foregroundStyle(fromUser ? theme.userText.opacity(0.7) : theme.muted)
                    } else {
                        ProgressView().tint(fromUser ? theme.userText.opacity(0.7) : theme.muted)
                    }
                }
                .frame(width: size.width, height: size.height)
                if let captionText {
                    Text(captionText)
                        .font(theme.font(Theme.body))
                        .foregroundStyle(fromUser ? theme.userText : theme.text)
                        .multilineTextAlignment(.leading)
                        .padding(.horizontal, Theme.bubbleHPad)
                        .padding(.vertical, Theme.bubbleVPad)
                        .frame(width: size.width, alignment: .leading)
                        .background(fill)
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(captionText == nil ? "Image, tap to view" : "Image with description, tap to view")
        .task(id: item.blobRef) { await load() }
        .fullScreenCover(item: $viewer) { v in
            MediaViewer(album: v.items, index: v.index, onClose: { viewer = nil })
                .environmentObject(model)
                .environment(\.theme, theme)
        }
    }

    private func load() async {
        if let (data, _) = await MediaFetch.bytes(for: item, model: model), let img = UIImage(data: data) {
            image = ImageTrim.trimmed(img)
            failed = false
        } else {
            failed = true
        }
    }
}

/// Trim a uniform border (a screenshot's empty page margin) off a picture —
/// the Mac's `trim_rect`: border colour = the corner pixel, a row / column is
/// border when every sampled pixel is within 10/255 of it, 2 % breathing pad,
/// and only when it removes ≥ 6 % of the area and keeps a quarter of each side.
enum ImageTrim {
    static func trimmed(_ image: UIImage) -> UIImage {
        guard let cg = image.cgImage, let rect = trimRect(cg) else { return image }
        guard let cropped = cg.cropping(to: rect) else { return image }
        return UIImage(cgImage: cropped, scale: image.scale, orientation: image.imageOrientation)
    }

    static func trimRect(_ cg: CGImage) -> CGRect? {
        let w = cg.width, h = cg.height
        guard w >= 32, h >= 32 else { return nil }
        // Sample on a ≤ 160 × 160 grid: draw into a small RGBA buffer.
        let sw = min(w, 160), sh = min(h, 160)
        var buf = [UInt8](repeating: 0, count: sw * sh * 4)
        guard let ctx = CGContext(data: &buf, width: sw, height: sh, bitsPerComponent: 8, bytesPerRow: sw * 4,
                                  space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        ctx.interpolationQuality = .none
        ctx.draw(cg, in: CGRect(x: 0, y: 0, width: sw, height: sh))
        func px(_ x: Int, _ y: Int) -> (UInt8, UInt8, UInt8) {
            let i = (y * sw + x) * 4
            return (buf[i], buf[i + 1], buf[i + 2])
        }
        // Bitmap contexts keep row 0 at the top of the picture.
        let bg = px(0, 0)
        func near(_ p: (UInt8, UInt8, UInt8)) -> Bool {
            abs(Int(p.0) - Int(bg.0)) <= 10 && abs(Int(p.1) - Int(bg.1)) <= 10 && abs(Int(p.2) - Int(bg.2)) <= 10
        }
        func rowIsBg(_ y: Int) -> Bool { (0..<sw).allSatisfy { near(px($0, y)) } }
        func colIsBg(_ x: Int) -> Bool { (0..<sh).allSatisfy { near(px(x, $0)) } }
        var top = 0
        while top < sh, rowIsBg(top) { top += 1 }
        if top >= sh { return nil }
        var bottom = sh
        while bottom > top, rowIsBg(bottom - 1) { bottom -= 1 }
        var left = 0
        while left < sw, colIsBg(left) { left += 1 }
        var right = sw
        while right > left, colIsBg(right - 1) { right -= 1 }
        // Back to image pixels, with the breathing pad.
        let fx = Double(w) / Double(sw), fy = Double(h) / Double(sh)
        let padX = Double(w) * 0.02, padY = Double(h) * 0.02
        let x0 = max(0, Double(left) * fx - padX), y0 = max(0, Double(top) * fy - padY)
        let x1 = min(Double(w), Double(right) * fx + padX), y1 = min(Double(h), Double(bottom) * fy + padY)
        let cw = x1 - x0, ch = y1 - y0
        if cw < Double(w) / 4 || ch < Double(h) / 4 { return nil }
        if (cw * ch) / (Double(w) * Double(h)) > 0.94 { return nil }
        return CGRect(x: x0.rounded(), y: y0.rounded(), width: cw.rounded(), height: ch.rounded())
    }
}

/// The composer reply preview's picture: a fixed square thumbnail of the quoted image,
/// loaded with the device bearer like `ImageBubble` (URLSession caches the bytes, so a
/// picture already in the transcript costs nothing). A blank tile until it arrives — the
/// row's height never depends on it.
struct ReplyThumb: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let projectId: Id
    let blobRef: String

    @State private var image: UIImage?

    var body: some View {
        ZStack {
            theme.field
            if let image {
                Image(uiImage: image).resizable().scaledToFill()
            } else {
                Image(systemName: "photo").font(.system(size: 10)).foregroundStyle(theme.muted)
            }
        }
        .frame(width: Theme.replyPreviewThumb, height: Theme.replyPreviewThumb)
        .clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous))
        .accessibilityHidden(true)
        .task(id: blobRef) { await load() }
    }

    private func load() async {
        guard let url = model.blobURL(projectId: projectId, key: blobRef) else { return }
        var req = URLRequest(url: url)
        for (k, v) in model.authHeaders { req.setValue(v, forHTTPHeaderField: k) }
        guard let (data, response) = try? await URLSession.shared.data(for: req),
              let http = response as? HTTPURLResponse, (200..<400).contains(http.statusCode) else { return }
        image = UIImage(data: data)
    }
}

/// Opening the viewer from a bubble or a tile: the chat's album at the item's
/// place for pictures and videos; the item alone for a file.
enum MediaOpen {
    @MainActor
    static func state(for item: MediaItem, chatId: Id, model: AppModel) -> MediaViewerState {
        if item.kind != .file {
            let album = MediaAlbum.items(messages: model.messages(of: chatId), slot: item.slot,
                                         projectName: { model.project($0)?.name })
            if let ix = album.firstIndex(where: { $0.id == item.id }) {
                return MediaViewerState(items: album, index: ix)
            }
        }
        return MediaViewerState(items: [item], index: 0)
    }
}

/// Document card (Grok: icon, title, domain) for a non-audio attachment.
/// Tap opens the media viewer on it (Open there = Quick Look, plus Download / Forward).
/// A video's typed description sits as a caption bar under the card.
struct FileCard: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let item: MediaItem
    let subtitle: String
    let chatId: Id
    let fromUser: Bool
    var caption: String? = nil

    @State private var viewer: MediaViewerState?

    private var captionText: String? {
        guard let caption else { return nil }
        let t = caption.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }

    var body: some View {
        Button {
            viewer = MediaOpen.state(for: item, chatId: chatId, model: model)
        } label: {
            // Grok's file card (web-08, measured): 61 pt tall, 20×27 icon at 15 pt,
            // text 16 pt after the icon, 17 pt title over a 15 pt muted subtitle.
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 14) {
                    RoundedRectangle(cornerRadius: 5, style: .continuous)
                        .fill(FileCard.kindColor(item, theme: theme))
                        .frame(width: 21, height: 27)
                        .overlay(Image(systemName: item.kind == .video ? "play.fill" : "doc.text").font(.system(size: 12, weight: .bold)).foregroundStyle(.black))
                    VStack(alignment: .leading, spacing: 2) {
                        Text(item.name).font(theme.font(Theme.body)).foregroundStyle(fromUser ? theme.userText : theme.text).lineLimit(1)
                        Text(subtitle).font(theme.font(Theme.small)).foregroundStyle(fromUser ? theme.userText.opacity(0.7) : theme.muted).lineLimit(1)
                    }
                }
                .padding(.leading, 15)
                .padding(.trailing, Theme.bubbleHPad + 4)
                .padding(.vertical, 13)
                if let captionText {
                    Text(captionText)
                        .font(theme.font(Theme.body))
                        .foregroundStyle(fromUser ? theme.userText : theme.text)
                        .multilineTextAlignment(.leading)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, Theme.bubbleHPad)
                        .padding(.bottom, Theme.bubbleVPad)
                        .padding(.top, 2)
                }
            }
            .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(fromUser ? theme.bubbleUser : theme.bubbleAssistant))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(item.blobRef == nil)
        .accessibilityLabel(item.blobRef == nil ? "\(item.name), unavailable" : "\(item.name), \(subtitle), tap to open")
        .fullScreenCover(item: $viewer) { v in
            MediaViewer(album: v.items, index: v.index, onClose: { viewer = nil })
                .environmentObject(model)
                .environment(\.theme, theme)
        }
    }

    /// The doc glyph's colour by kind (Mac `kind_color`): Grok's Docs yellow for
    /// documents, PDF red, Sheets green, Slides orange; the rest neutral.
    static func kindColor(_ item: MediaItem, theme: Theme) -> Color {
        let ext = (item.name as NSString).pathExtension.lowercased()
        switch ext {
        case "pdf": return Color(hex: "#E53E3E")!
        case "docx", "doc", "odt", "rtf", "md", "txt": return Color(hex: "#FFB000")!
        case "xlsx", "xls", "csv", "ods": return Color(hex: "#2EB86A")!
        case "pptx", "ppt", "key", "odp": return Color(hex: "#F28B2E")!
        default: return item.mime == "application/pdf" ? Color(hex: "#E53E3E")! : theme.muted
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
