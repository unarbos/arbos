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
    /// `> ` lines, parsed again inside: a quote holds any block, including another quote
    /// ([feedback chat-markdown-ios-2026-10-27]).
    indirect case quote([Block])
    case bullets([String])
    case numbered([String])
    case code(lang: String, code: String)
    /// GitHub-style pipe table: header cells, then body rows (each padded or cut to the
    /// header's width by the renderer).
    case table(header: [String], rows: [[String]])
    /// Display math on its own line(s): `$$ … $$` or `\[ … \]`; `source` is the LaTeX between
    /// the delimiters ([feedback math-render-ios-2026-10-26]).
    case math(source: String)
    /// A chart bubble's card ([feedback chart-bubble-ios-2026-10-26]): not markdown, but one
    /// block of the agent's bubble like a code card. `spec` is the `chart` part's JSON, `table`
    /// its markdown table (what a copy carries).
    case chart(partId: String, spec: String, table: String)
    /// A link preview card ([feedback link-embeds-ios-2026-10-27]): the relay's `link_preview`
    /// part as one block of the bubble, under the prose that held the link.
    case linkPreview(partId: String, preview: LinkPreview)
}

/// Grok Bot's quote header inside a reply bubble: speech-bubble glyph, then the replied-to
/// message's first words in quotes, one line, truncated with "…". Tapping it reveals the original.
struct Quote: Equatable, Hashable {
    /// The message replied to (`replyTo` on the row, or `replyRef.messageId`); empty on a
    /// forward header.
    var messageId: Id
    var text: String
    /// Set on a forwarded message: the header reads "Forwarded from <name>" with the
    /// forward glyph instead of a reply quote, and does not scroll anywhere.
    var forwardedFrom: String? = nil
    /// Reply in DM (#1836): the original lives in another chat — its address and the
    /// "Replying to <who> in <chat>" line. `nil` for an in-chat reply.
    var source: ReplyRef? = nil

    /// The header for a forwarded message (`forwardedFrom` on the row).
    static func forwarded(from name: String) -> Quote {
        Quote(messageId: "", text: name, forwardedFrom: name)
    }

    /// The quote for a cross-chat reply: drawn from the row's own snapshot, so it needs
    /// nothing from the source chat.
    static func forReplyRef(_ r: ReplyRef) -> Quote {
        let flat = oneLine(r.text)
        let text = flat.isEmpty ? "Message" : firstWords(flat, maxChars: maxChars)
        return Quote(messageId: r.messageId, text: text, source: r)
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
            case .quote(let inner): pieces = [oneLine(blocks: inner)]
            case .bullets(let items), .numbered(let items): pieces = items
            case .code(_, let code): pieces = [code]
            case .table(let header, let rows): pieces = header + rows.flatMap { $0 }
            case .math(let source): pieces = [MathText.plain(source)]
            case .chart(_, _, let table): pieces = [table]
            case .linkPreview(_, let p): pieces = [p.oneLine]
            }
            for piece in pieces {
                words.append(contentsOf: Markdown.plain(piece).split(whereSeparator: \.isWhitespace))
            }
        }
        return words.joined(separator: " ")
    }

    /// `oneLine` over blocks already split (a quote's inner blocks).
    private static func oneLine(blocks: [Block]) -> String {
        blocks.map { block -> String in
            switch block {
            case .paragraph(let t), .heading(_, let t): return Markdown.plain(t)
            case .quote(let inner): return oneLine(blocks: inner)
            case .bullets(let items), .numbered(let items): return items.map(Markdown.plain).joined(separator: " ")
            case .code(_, let code): return code
            case .table(let header, let rows): return (header + rows.flatMap { $0 }).map(Markdown.plain).joined(separator: " ")
            case .math(let source): return MathText.plain(source)
            case .chart(_, _, let table): return table
            case .linkPreview(_, let p): return p.oneLine
            }
        }.joined(separator: " ")
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
    /// My bubble; `delivery` is its opacity (pending dimmed, delivered / failed full).
    case user(text: String, delivery: Delivery, quote: Quote?, preview: LinkPreview? = nil)
    /// Another person's message in a people chat: left bubble with avatar; the name above
    /// only when the sender changed (`named`, Grok Bot's group style) and the chat names its
    /// senders at all (group / channel, `rows(multiParty:)`); the name sits inside the bubble
    /// (Jacob, 2026-10-23). No time inside any bubble: the time is the transcript's
    /// drag-to-reveal only (Jacob, 2026-10-24, docs/mac-design-rules.md).
    case other(text: String, handle: String, avatar: String?, named: Bool, quote: Quote?, preview: LinkPreview? = nil)
    case assistant(blocks: [Block], live: Bool, caption: String?, quote: Quote? = nil)
    /// Shared agent in a people chat: same shape as `.other` (avatar + bubble, name on each
    /// reply — consecutive answers from the same agent must not merge, #1447). No
    /// "answering @x · owned by @y" meta — ownership lives in the profile pane
    /// (Jacob, 2026-10-22). `agentId` opens that pane from the avatar / name.
    /// `quote` is the user message this answers (`inReplyTo`).
    case member(blocks: [Block], live: Bool, handle: String, name: String?, avatar: String?, named: Bool, agentId: String?, quote: Quote?)
    /// Centred muted line: hand-offs ("Messaged ◉ Affine"), stops, dates.
    case system(icon: String?, text: String, avatarProject: Id?)
    /// A run of messages from someone I blocked, folded into one muted line ("3 messages
    /// hidden because you blocked @sam · Unblock") — the blocker alone sees this
    /// ([feedback blocked-state-ios-2026-10-26], Jacob 2026-10-26).
    case hidden(count: Int, sender: String, handle: String, name: String?)
    /// Grok Bot question card: bold question, × top-right, lettered answer rows.
    /// `questionId` is nil for a legacy `waiting_user` status line (no `question` part).
    /// `lane`: in a people chat the card sits after the sender's avatar lane, like the bubble it
    /// belongs to (#1440 follow-up: the card must share the bubble's left edge and cap width).
    /// `shellCard`: a destructive-shell card with standing options (#1544).
    case question(questionId: String?, text: String, options: [ChoiceOption], multi: Bool, picked: [String], open: Bool, lane: Bool, shellCard: Bool = false, offer: SecretOffer? = nil)
    /// Secret hand-off: the message's value moved into the vault — "Secret saved as NAME"
    /// in the sender's slot (mine right, another member's left under their name). Nothing
    /// here is the value; nothing to copy or search.
    case secretGhost(name: String, mine: Bool, handle: String, avatar: String?, named: Bool)
    /// A file card; tap opens the media viewer on it (Open → Quick Look).
    /// `caption` is an optional description bar under a video when the sender typed
    /// one with the attachment — not a separate text bubble.
    /// `forwardedFrom`: the "Forwarded from <name>" header at the top of the card when
    /// this is the first media of a forwarded message
    /// ([feedback forward-attachments-ios-2026-10-27]).
    /// `sender` / `named`: someone else's card in a people chat carries their avatar beside it
    /// and, on the message's first card when the run changes sender, their name above it —
    /// a picture is a message like any other (Jacob's Scout case).
    case file(item: MediaItem, subtitle: String, caption: String?, forwardedFrom: String? = nil,
              sender: MediaSender? = nil, named: Bool = false)
    case voiceMemo(name: String, projectId: Id, ref: String?, fromUser: Bool)
    /// A picture; tap opens the media viewer at its place in the chat's album.
    /// `caption` is the description bar under the picture when the send carried text.
    /// `lane`: in a people chat someone else's (person or agent) picture sits after the
    /// sender's avatar lane, like their bubble (Mac `RowKind::Attachment.lane`).
    /// `forwardedFrom`: the "Forwarded from <name>" header above the picture when this is
    /// the first media of a forwarded message ([feedback forward-attachments-ios-2026-10-27]).
    /// `source`: GIF-picker attribution ("via <provider>").
    case image(item: MediaItem, fromUser: Bool, caption: String?, lane: Bool, source: AttachmentSource?,
               forwardedFrom: String? = nil, sender: MediaSender? = nil, named: Bool = false)
    /// "Forwarded from <name>" under a forwarded attachment (media-viewer-2026-10-23).
    case forwardHint(text: String, fromUser: Bool)
    case error(code: String, message: String)
}

/// Who sent a picture / file row that is not mine in a people chat — the same identity its
/// text bubble resolves ([feedback forward-attachments-ios-2026-10-27]).
enum MediaSender: Equatable {
    /// Another person: their handle and picture URL (`.other` bubble fields).
    case person(handle: String, avatar: String?)
    /// An agent (`.member` bubble fields): project id when known, handle, name, picture.
    case agent(id: String?, handle: String, name: String?, avatar: String?)
}

struct Row: Identifiable, Equatable {
    var id: String
    var messageId: Id
    var role: Role
    var kind: RowKind
    /// Emoji reactions shown under this bubble (stamped on the last prose/media row of a message).
    var reactions: [MessageReaction] = []
    /// How many messages reply to this one via `replyTo` (long-press Reply); shown top-right when > 0.
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
    /// Settings → Appearance → Link previews: off hides every card (plain links stay).
    /// Set by `AppModel.linkPreviews`; a static so the row builder needs no model.
    nonisolated(unsafe) static var linkPreviewsEnabled = true

    static func rows(_ messages: [Message], projectId: Id, subagentNames: [String: String] = [:], me: String? = nil, people: Bool = false, agentNames: [String: String] = [:], multiParty: Bool = false, memberNames: [String: String] = [:], hiddenSenders: Set<String> = []) -> [Row] {
        rows(messages, byId: index(messages), projectId: projectId, subagentNames: subagentNames, me: me, people: people, agentNames: agentNames, multiParty: multiParty, memberNames: memberNames, hiddenSenders: hiddenSenders)
    }

    /// Same, with the caller's `index(messages)` (the chat keeps it for its own lookups).
    /// `multiParty`: the chat names its senders — a group or a server channel (people and
    /// agents alike); a 1:1 chat (me ↔ person, me ↔ agent) labels nobody (Jacob, 2026-10-23).
    /// `memberNames`: the chat's people by user id → current display name (me included), so a
    /// membership line names its actor as the roster does now, not as the row was written.
    /// `hiddenSenders`: `user:<id>` keys of people I blocked — their messages fold into one
    /// `.hidden` row per run instead of bubbles, and carry no date separator.
    static func rows(_ messages: [Message], byId: [Id: Message], projectId: Id, subagentNames: [String: String] = [:], me: String? = nil, people: Bool = false, agentNames: [String: String] = [:], multiParty: Bool = false, memberNames: [String: String] = [:], hiddenSenders: Set<String> = []) -> [Row] {
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
            // A background job's row (duplex §2.3, `job` + `hidden`) is wordless by design:
            // its status parts feed the presence line and the tasks sheet, never a bubble —
            // not while it runs, not the "no answer" stock line when it ends. One exception:
            // a file or picture the worker delivered onto that row (agentd ≤ 0.1.150 put
            // `deliver_file` there — Jacob's SN120 chart, 2026-10-27, "the chart is in the
            // card above" over nothing) still draws as its card, so a posted file is never
            // invisible on a box that has not upgraded.
            if message.hidden || message.job != nil {
                // The card carries the agent's avatar and name like its bubbles do
                // ([feedback forward-attachments-ios-2026-10-27]). A worker row from an older
                // box says a bare `agent` with no handle; then the agent's own ack or result
                // row answering the same ask lends its attribution.
                let who = hiddenRowAgent(message, in: messages)
                let inLane = people && who.senderHandle != nil
                let whoId: String? = who.sender.hasPrefix("agent:") ? String(who.sender.dropFirst(6)) : nil
                let cards = message.parts.compactMap { part -> (String, Attachment)? in
                    if case .attachment(let id, let a) = part { return (id, a) }
                    return nil
                }
                for (ix, (id, a)) in cards.enumerated() {
                    rows.append(attachmentRow(id: "\(message.id)#\(id)", message: message, a: a,
                                              projectId: projectId, fromUser: false, agentNames: agentNames,
                                              lane: inLane,
                                              sender: inLane ? .agent(id: whoId, handle: who.senderHandle ?? "",
                                                                      name: who.senderName, avatar: who.senderAvatar) : nil,
                                              named: multiParty && inLane && ix == 0))
                }
                continue
            }
            // A background worker's spawn row (`worker: true`, Grok parity trait 15) draws nothing at
            // all — no hand-off line, no legacy status line, no date separator. Its `activity` reaches
            // the working line through `AppModel.currentActivity`. Skipping only the hand-off name
            // (6e7261a5) left the legacy `.status(.handoff)` fallback drawing "Messaged ◉ <worker>".
            if message.worker && message.turn?.isProgress == true { continue }
            // Someone I blocked: fold the run into one hidden line, no bubble, no date line.
            if message.role == .user, hiddenSenders.contains(message.sender) {
                if let last = rows.indices.last,
                   case .hidden(let count, let sender, let handle, let name) = rows[last].kind,
                   sender == message.sender {
                    rows[last].kind = .hidden(count: count + 1, sender: sender, handle: handle, name: name)
                } else {
                    rows.append(Row(id: "\(message.id)#hidden", messageId: message.id, role: .system,
                                    kind: .hidden(count: 1, sender: message.sender,
                                                  handle: message.senderHandle ?? "someone", name: message.senderName),
                                    createdAt: message.createdAt))
                }
                prevSender = nil
                continue
            }
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
                // Secret hand-off: a redacted row is the ghost bubble and nothing else.
                if let ghostName = message.parts.lazy.compactMap(\.secretRedactedName).first {
                    chunk.append(Row(id: "\(message.id)#ghost", messageId: message.id, role: mine ? .user : .project,
                                     kind: .secretGhost(name: ghostName, mine: mine, handle: message.senderHandle ?? "someone",
                                                        avatar: message.senderAvatar, named: named)))
                    break
                }
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
                var text = message.plainText
                // The link preview card ([feedback link-embeds-ios-2026-10-27]); a message that
                // was only the link shows the card alone (iMessage's rule).
                var preview: LinkPreview? = Self.linkPreviewsEnabled ? message.linkPreview?.preview : nil
                preview?.slot = projectId
                if let p = preview, p.isLinkOnly(text) { text = "" }
                // A media-viewer forward's whole text is the "Forwarded from …" line: a
                // hint under the card, not a bubble of its own.
                let forwarded = hasAttachment ? MediaAlbum.forwardedFrom(text) : nil
                // A bubble-menu forward carries `forwardedFrom`; with media the header sits
                // once, on the first card, and the caption / note under it stays plain
                // ([feedback forward-attachments-ios-2026-10-27]).
                let forwardedBy = (message.forwardedFrom?.trimmingCharacters(in: .whitespacesAndNewlines))
                    .flatMap { $0.isEmpty ? nil : $0 }
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
                    // Another person's picture / file sits left like their bubble, with their
                    // avatar beside it and their name on the first card when the run changes
                    // sender ([feedback forward-attachments-ios-2026-10-27]).
                    chunk.append(attachmentRow(id: "\(message.id)#\(part.id)", message: message, a: part.a,
                                               projectId: projectId, fromUser: mine, agentNames: agentNames,
                                               caption: cap, lane: !mine,
                                               forwardedFrom: i == 0 && forwarded == nil ? forwardedBy : nil,
                                               sender: mine ? nil : .person(handle: message.senderHandle ?? "someone",
                                                                            avatar: message.senderAvatar),
                                               named: named && i == 0))
                }
                if let forwarded {
                    chunk.append(Row(id: "\(message.id)#forwarded", messageId: message.id, role: mine ? .user : .project,
                                     kind: .forwardHint(text: forwarded, fromUser: mine)))
                }
                // Generic labels ("Image", "Photo", …) are not prose — the attachment
                // row already carries the media. Showing them as a bubble is noise.
                // A real description on a picture/video is the caption bar on that row.
                if (!text.isEmpty || preview != nil), forwarded == nil, mediaCaption == nil, !(hasImage && Self.isGenericMediaLabel(text)) {
                    // A forward carries its "Forwarded from" header where a reply carries its
                    // quote — on the first media card when the message has any, else here.
                    let quote: Quote?
                    if let by = forwardedBy {
                        quote = mediaParts.isEmpty ? .forwarded(from: by) : nil
                    } else if let r = message.replyRef {
                        // Reply in DM: the quote comes from the row's own snapshot.
                        quote = .forReplyRef(r)
                    } else {
                        quote = message.replyTo.flatMap { $0.isEmpty ? nil : Quote.forReply($0, in: byId) }
                    }
                    if mine {
                        chunk.append(Row(id: "\(message.id)#user", messageId: message.id, role: .user,
                                         kind: .user(text: text, delivery: Delivery.of(message), quote: quote, preview: preview)))
                    } else {
                        // The first card already names the sender.
                        chunk.append(Row(id: "\(message.id)#other", messageId: message.id, role: .project,
                                         kind: .other(text: text, handle: message.senderHandle ?? "someone", avatar: message.senderAvatar,
                                                      named: named && mediaParts.isEmpty, quote: quote, preview: preview)))
                    }
                }
            case .system:
                // Call lines and other system notes: one muted centred line.
                // Admin deletion audits stay out of the chat (GroupActivity screen only).
                // A membership line is worded from its `notice` facts and the live roster;
                // the stored words are the fallback when someone named is unknown here.
                let live = message.notice?.text { kind, id in
                    let n = (kind == "agent" ? agentNames[id] : memberNames[id])?.trimmingCharacters(in: .whitespaces)
                    return (n?.isEmpty ?? true) ? nil : n
                }
                let text = (live ?? message.plainText).trimmingCharacters(in: .whitespacesAndNewlines)
                if !text.isEmpty, !GroupActivity.isAuditLine(text) {
                    let icon = text.hasPrefix("Call ended") ? "phone.down.fill" : (text.contains(" call") ? "phone.fill" : nil)
                    chunk.append(Row(id: "\(message.id)#system", messageId: message.id, role: .system,
                                     kind: .system(icon: icon, text: text, avatarProject: nil)))
                }
            case .assistant, .project:
                // Hand-off: Grok shows "Messaged ◉ <bot>" / "N messages with ◉ A, ◉ B and ◉ C"
                // as a system line. Consecutive hand-offs merge below.
                // A background worker's spawn row draws nothing (`worker`, feedback
                // `hide-worker-rows-and-tz-2026-10-24`); its `activity` still feeds the
                // working line through `AppModel.currentActivity`.
                let handoffName = (message.turn == .new || message.turn?.isProgress == true) && !message.worker
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
                // The sender's name goes on the message's first row: a picture before the
                // prose takes it, and the first text block then does not repeat it
                // ([feedback forward-attachments-ios-2026-10-27]).
                var nameTaken = false
                for (pix, part) in message.parts.enumerated() {
                    switch part {
                    case .text(let id, let text):
                        // Polish on display (Mac parity): glued "findingsDon't" / "have.Checking"
                        // and a trailing tool-narration sentence still read as chat.
                        let blocks = Self.proseBlocks(text, people: people)
                        if blocks.isEmpty { continue }
                        let live = streaming && pix == lastTextPart
                        if people, let h = message.senderHandle {
                            let names = named && pix == firstTextPart && !nameTaken
                            nameTaken = nameTaken || names
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                             kind: .member(blocks: blocks, live: live, handle: h, name: message.senderName,
                                                           avatar: message.senderAvatar, named: names,
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
                                                         lane: people && message.senderHandle != nil, shellCard: q.shellCard,
                                                         offer: q.secretOffer)))
                    case .status(let id, .handoff, let text):
                        // Legacy status line; the turn-based row above already covers it.
                        if handoffName == nil {
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .system,
                                             kind: .system(icon: nil, text: handoffText(text), avatarProject: nil)))
                        }
                    case .status(let id, .stopped, _):
                        chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .system,
                                         kind: .system(icon: "stop.circle", text: "Stopped", avatarProject: nil)))
                    case .attachment(_, let a) where a.chartPart.map(message.hasChartPart) == true:
                        // The chart's fallback picture: an older app shows it, this one draws
                        // the chart instead ([feedback chart-bubble-ios-2026-10-26]).
                        break
                    case .attachment(let id, let a):
                        // A people-chat agent's picture / file sits in its lane with its avatar
                        // and names it when it is the message's first row (Scout's photo in
                        // Arbonites drew as a bare picture before).
                        let inLane = people && message.senderHandle != nil
                        let names = inLane && named && !nameTaken
                        nameTaken = nameTaken || names
                        chunk.append(attachmentRow(id: "\(message.id)#\(id)", message: message, a: a,
                                                   projectId: projectId, fromUser: false, agentNames: agentNames,
                                                   lane: inLane,
                                                   sender: inLane ? .agent(id: agentId, handle: message.senderHandle ?? "",
                                                                           name: message.senderName, avatar: message.senderAvatar) : nil,
                                                   named: names))
                    case .chart(let id, _, let spec, let table, _):
                        // A chart bubble's card: one block of the bubble, like a code card. It
                        // joins the caption's bubble when that is the row just before it.
                        let block: Block = .chart(partId: id, spec: spec, table: table)
                        if let last = chunk.indices.last, chunk[last].messageId == message.id,
                           let joined = Self.appending(block, to: chunk[last].kind) {
                            chunk[last].kind = joined
                        } else if people, let h = message.senderHandle {
                            let names = named && !nameTaken
                            nameTaken = nameTaken || names
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                             kind: .member(blocks: [block], live: false, handle: h, name: message.senderName,
                                                           avatar: message.senderAvatar, named: names,
                                                           agentId: agentId, quote: nil)))
                        } else {
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                             kind: .assistant(blocks: [block], live: false, caption: nil, quote: nil)))
                        }
                    case .linkPreview(let id, var p) where Self.linkPreviewsEnabled && message.linkPreview?.id == id:
                        // The link preview card: one block under the prose, joining the
                        // bubble before it ([feedback link-embeds-ios-2026-10-27]).
                        p.slot = projectId
                        let block: Block = .linkPreview(partId: id, preview: p)
                        if let last = chunk.indices.last, chunk[last].messageId == message.id,
                           let joined = Self.appending(block, to: chunk[last].kind) {
                            chunk[last].kind = joined
                        } else if people, let h = message.senderHandle {
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                             kind: .member(blocks: [block], live: false, handle: h, name: message.senderName,
                                                           avatar: message.senderAvatar, named: named && firstTextPart == nil,
                                                           agentId: agentId, quote: nil)))
                        } else {
                            chunk.append(Row(id: "\(message.id)#\(id)", messageId: message.id, role: .assistant,
                                             kind: .assistant(blocks: [block], live: false, caption: nil, quote: nil)))
                        }
                    case .linkPreview:
                        break
                    case .error(let id, let code, let text):
                        errors.append((id: id, code: code, text: text))
                    case .status, .tool, .subagentResult, .reasoning, .secretRedacted, .unknown:
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
                    case .tool, .attachment, .question, .subagentResult, .reasoning, .secretRedacted, .unknown, .chart, .linkPreview: return false
                    }
                }
                let failed = !errors.isEmpty || message.status == .error
                // A hand-off row (a subagent brief, an agent-to-agent send) is its own line
                // ("Messaged ◉ X"); it never gets the stock line, even with no parts (#1466).
                let handoffRow = handoffName != nil
                // The stock line is for a turn that said NOTHING. A turn that answered with a
                // `send_message` bubble and ended on the fresh result row that bubble opened
                // (wordless, "Working…") must not get "didn't answer" under its own answer
                // (Jacob, "run bubble sort", 2026-10-25). Another visible row of the same turn
                // = an assistant row answering the same user message with words or a card.
                let turnAnsweredElsewhere: Bool = {
                    guard !failed, let ask = message.inReplyTo else { return false }
                    return messages.contains { other in
                        other.id != message.id && other.role == .assistant && other.inReplyTo == ask
                            && (!other.plainText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                || other.parts.contains { part in
                                    switch part {
                                    case .question, .attachment, .chart, .linkPreview: return true
                                    default: return false
                                    }
                                })
                    }
                }()
                if showsNothing, finished, !streaming, !handoffRow, !turnAnsweredElsewhere, failed || onlyEmpty {
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

    /// A chart block joins the bubble row just before it (the caption's), so the card sits
    /// under the prose like a code card; `nil` when that row is not a bubble of blocks.
    static func appending(_ block: Block, to kind: RowKind) -> RowKind? {
        switch kind {
        case .assistant(let blocks, let live, let caption, let quote):
            return .assistant(blocks: blocks + [block], live: live, caption: caption, quote: quote)
        case .member(let blocks, let live, let handle, let name, let avatar, let named, let agentId, let quote):
            return .member(blocks: blocks + [block], live: live, handle: handle, name: name, avatar: avatar,
                           named: named, agentId: agentId, quote: quote)
        default:
            return nil
        }
    }

    /// Who a hidden job row's card is drawn under: the row itself when it names its agent
    /// (`agent:<id>` + handle), else the agent's ack / result row answering the same ask —
    /// agentd ≤ 0.1.150 wrote worker rows as a bare `agent` with no handle.
    static func hiddenRowAgent(_ message: Message, in messages: [Message]) -> Message {
        if message.sender.hasPrefix("agent:"), message.senderHandle != nil { return message }
        guard let ask = message.inReplyTo, !ask.isEmpty else { return message }
        return messages.first {
            $0.id != message.id && ($0.role == .assistant || $0.role == .project)
                && $0.inReplyTo == ask && $0.sender.hasPrefix("agent:") && $0.senderHandle != nil
        } ?? message
    }

    private static func attachmentRow(id: String, message: Message, a: Attachment, projectId: Id, fromUser: Bool,
                                      agentNames: [String: String], caption: String? = nil,
                                      lane: Bool = false, forwardedFrom: String? = nil,
                                      sender: MediaSender? = nil, named: Bool = false) -> Row {
        guard let kind = MediaAlbum.kind(of: a) else {
            return Row(id: id, messageId: message.id, role: fromUser ? .user : .assistant,
                       kind: .voiceMemo(name: a.name, projectId: projectId, ref: a.blobRef, fromUser: fromUser))
        }
        let item = MediaAlbum.item(id: id, message: message, attachment: a, kind: kind, slot: projectId,
                                   projectName: { agentNames[$0] })
        if kind == .image {
            return Row(id: id, messageId: message.id, role: fromUser ? .user : .assistant,
                       kind: .image(item: item, fromUser: fromUser, caption: caption, lane: lane, source: a.source,
                                    forwardedFrom: forwardedFrom, sender: sender, named: named))
        }
        // Video shows as a file card in the bubble (poster + Play live in the viewer).
        // A typed description rides the card as a caption bar (pictures/videos only).
        return Row(id: id, messageId: message.id, role: fromUser ? .user : .assistant,
                   kind: .file(item: item, subtitle: subtitle(for: a),
                               caption: kind == .video ? caption : nil, forwardedFrom: forwardedFrom,
                               sender: sender, named: named))
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

    /// The blocks of a message, from the shared parser (`arbos_core::markdown` through
    /// `markdownBlocks`): the exact grammar the Mac renders. The FFI hands quotes back flat
    /// with a depth per block; this rebuilds the tree. Cached by text — a long group's bodies
    /// are re-evaluated often, and the parse crosses the FFI.
    static func blocks(_ text: String) -> [Block] {
        if let hit = blockCache.object(forKey: text as NSString) { return hit.blocks }
        let blocks = nest(markdownBlocks(text: text))
        blockCache.setObject(BlockBox(blocks), forKey: text as NSString)
        return blocks
    }

    private final class BlockBox {
        let blocks: [Block]
        init(_ blocks: [Block]) { self.blocks = blocks }
    }

    private static let blockCache: NSCache<NSString, BlockBox> = {
        let cache = NSCache<NSString, BlockBox>()
        cache.countLimit = 6_000
        return cache
    }()

    /// The quote tree from the flat FFI list: consecutive blocks deeper than `depth` form one
    /// `.quote` at this level, parsed again one level down.
    static func nest(_ flat: [MarkdownBlock], depth: UInt8 = 0) -> [Block] {
        var out: [Block] = []
        var i = 0
        while i < flat.count {
            let d = quoteDepth(flat[i])
            if d > depth {
                var j = i
                while j < flat.count, quoteDepth(flat[j]) > depth { j += 1 }
                out.append(.quote(nest(Array(flat[i..<j]), depth: depth + 1)))
                i = j
                continue
            }
            switch flat[i] {
            case .paragraph(_, let text): out.append(.paragraph(text))
            case .heading(_, let level, let text): out.append(.heading(level: Int(level), text: text))
            case .bullets(_, let items): out.append(.bullets(items))
            case .numbered(_, let items): out.append(.numbered(items))
            case .code(_, let lang, let code): out.append(.code(lang: lang, code: code))
            case .table(_, let header, let rows): out.append(.table(header: header, rows: rows))
            case .math(_, let source): out.append(.math(source: source))
            }
            i += 1
        }
        return out
    }

    private static func quoteDepth(_ block: MarkdownBlock) -> UInt8 {
        switch block {
        case .paragraph(let d, _), .heading(let d, _, _), .bullets(let d, _), .numbered(let d, _),
             .code(let d, _, _), .table(let d, _, _), .math(let d, _):
            return d
        }
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

    /// The text inline markdown renders to, markers removed (`[Name](url)` → `Name`,
    /// `$O(n^2)$` → `O(n²)`).
    static func plain(_ text: String) -> String {
        markdownSpans(text: text).map { span in
            span.math ? MathText.plain(span.text) : span.text
        }.joined()
    }

    /// Typeset TeX as attributed text: variables italic in the serif face, digits and
    /// operators upright, `\mathbf` bold — KaTeX's look in Grok Bot, at `size` points.
    static func math(_ source: String, color: Color, size: CGFloat) -> AttributedString {
        var out = AttributedString()
        for run in MathText.typeset(source) {
            var piece = AttributedString(run.text)
            piece.swiftUI.foregroundColor = color
            piece.swiftUI.font = mathFont(size, run.style)
            out.append(piece)
        }
        return out
    }

    static func mathFont(_ size: CGFloat, _ style: MathText.Style) -> Font {
        switch style {
        case .italic: return .system(size: size, design: .serif).italic()
        case .upright: return .system(size: size, design: .serif)
        case .bold: return .system(size: size, weight: .bold, design: .serif)
        }
    }

    /// Display math is this much larger than body text.
    static let displayMathScale: CGFloat = 1.15

    /// Inline markdown with Mac-parity link paint and Grok-style code chips:
    /// accent + underline on links; monospaced (a point under body) on a soft
    /// wash for ``code``; `~~strike~~` struck through; a `||spoiler||` a solid pill
    /// until tapped (`SpoilerState`). Bare `http(s)://` and `www.` URLs are linked
    /// too. The runs come from the shared parser (`markdownSpans`), so a bubble here
    /// reads exactly as on the Mac.
    static func inline(_ text: String, color: Color, link: Color, codeChip: Color) -> AttributedString {
        // Parsed once per text, palette and spoiler state: the parse crosses the FFI on
        // every bubble body, and a long group re-evaluates many bodies at once
        // (arbonites-chat-lag). NSCache: bounded, dropped under memory pressure.
        let revealed = SpoilerState.shared.revealedIndices(in: text)
        let key = "\(text)\u{1}\(color)\u{1}\(link)\u{1}\(codeChip)\u{1}\(revealed)" as NSString
        if let hit = inlineCache.object(forKey: key) { return hit.value }
        let attr = inlineUncached(text, color: color, link: link, codeChip: codeChip, revealed: revealed)
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

    /// How much of the ink a hidden spoiler's pill takes, and an open one's wash.
    static let spoilerHiddenAlpha: Double = 0.9
    static let spoilerOpenAlpha: Double = 0.16

    private static func inlineUncached(_ text: String, color: Color, link: Color, codeChip: Color,
                                       revealed: Set<UInt16>) -> AttributedString {
        var attr = AttributedString()
        for span in markdownSpans(text: text) {
            if span.math {
                var piece = math(span.text, color: color, size: Theme.body)
                if let href = span.href, let url = URL(string: href) { piece.link = url }
                attr.append(piece)
                continue
            }
            var piece = AttributedString(span.text)
            piece.swiftUI.foregroundColor = color
            if span.code {
                // Default markdown mono ignores the bubble's 17 pt body and looks
                // undersized next to SF Pro; pin size and chip like Mac / M-101.
                piece.swiftUI.font = .system(size: Theme.code, design: .monospaced)
                piece.swiftUI.backgroundColor = codeChip
                piece.inlinePresentationIntent = .code
            } else if span.bold && span.italic {
                piece.swiftUI.font = .system(size: Theme.body, weight: .bold).italic()
            } else if span.bold {
                piece.swiftUI.font = .system(size: Theme.body, weight: .bold)
            } else if span.italic {
                piece.swiftUI.font = .system(size: Theme.body).italic()
            }
            if span.strike {
                piece.strikethroughStyle = .single
            }
            if span.link, let href = span.href, let url = URL(string: href) {
                piece.link = url
                piece.swiftUI.foregroundColor = link
                piece.underlineStyle = .single
            }
            if span.spoiler != 0 {
                let open = revealed.contains(span.spoiler)
                // Ink at low alpha on the open wash; the pill hides the glyphs until then.
                piece.swiftUI.backgroundColor = color.opacity(open ? spoilerOpenAlpha : spoilerHiddenAlpha)
                if !open {
                    piece.swiftUI.foregroundColor = .clear
                    piece.underlineStyle = nil
                    piece.link = SpoilerState.url(text: text, index: span.spoiler)
                }
            }
            attr.append(piece)
        }
        return attr
    }

}

/// Which `||spoilers||` this viewer has tapped open, per launch: nothing is stored, and
/// another viewer of the same chat sees them hidden ([feedback chat-markdown-ios-2026-10-27]).
/// A spoiler is named by its text's hash and its index in that text (`MarkdownSpan.spoiler`).
final class SpoilerState: ObservableObject {
    static let shared = SpoilerState()

    @Published private(set) var revealed: Set<String> = []

    static func key(text: String, index: UInt16) -> String {
        "\(text.hashValue)#\(index)"
    }

    /// The link a hidden spoiler carries; `ChatView` routes a tap on it here.
    static func url(text: String, index: UInt16) -> URL? {
        var parts = URLComponents()
        parts.scheme = "arbos"
        parts.host = "spoiler"
        parts.path = "/" + key(text: text, index: index)
        return parts.url
    }

    /// The spoiler key in `url` when it is one (`arbos://spoiler/<key>`).
    static func key(from url: URL) -> String? {
        guard url.scheme == "arbos", url.host == "spoiler" else { return nil }
        let key = url.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        return key.isEmpty ? nil : key
    }

    func toggle(_ key: String) {
        if revealed.contains(key) { revealed.remove(key) } else { revealed.insert(key) }
    }

    /// The spoiler indices of `text` that are open.
    func revealedIndices(in text: String) -> Set<UInt16> {
        let prefix = "\(text.hashValue)#"
        var out: Set<UInt16> = []
        for key in revealed where key.hasPrefix(prefix) {
            if let n = UInt16(key.dropFirst(prefix.count)) { out.insert(n) }
        }
        return out
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
    /// A people chat (DM / group / channel): bubbles take `Theme.peopleBubbleInset`
    /// off the screen instead of the agent chat's `Theme.bubbleInset`
    /// ([feedback chat-density-ios-2026-10-26]).
    var people: Bool = false
    /// How far the transcript has been dragged left to reveal iMessage-style times (0…`timeRevealWidth`).
    var timeReveal: CGFloat = 0
    /// A tap on a reply bubble's quote header: the chat scrolls to this message id.
    var onQuoteTap: ((Quote) -> Void)? = nil
    /// Briefly tinted after a quote tap landed on this row's message.
    var highlighted: Bool = false
    /// Search-in-chat hit: soft accent wash so the keyword match stands out while
    /// searching ([feedback app-01M5Z0GE2GV4ZJ5RH28QBT7H12]).
    var searchHit: Bool = false
    /// A message row: swipe right on it replies (the long-press Reply item's twin;
    /// `BubbleSwipe`). False on system / channel / non-message rows.
    var canReply: Bool = false
    /// Reply swipe (owned by the transcript's one recognizer, `TranscriptSwipeHost`): how far
    /// this bubble follows the finger to the right, and whether releasing now replies.
    var replyDrag: CGFloat = 0
    var replyArmed: Bool = false
    /// Shared mention table from ChatView (built once per directory sync).
    let mentions: MentionTable
    /// When true (held-bubble overlay), list everyone under every reaction chip so
    /// a message long-press shows who used each emoji without an extra tap.
    var showReactionSenders: Bool = false

    /// Chip long-press: which emoji's sender list is open under the chips.
    @State private var selectedReactionEmoji: String? = nil
    /// The agent-error note's Details disclosure.
    @State private var errorDetailsOpen = false

    /// Trailing column width for swipe-revealed times (fits "12:59 PM").
    static let timeRevealWidth: CGFloat = 56

    /// Skip re-drawing when the parent re-evaluates for an unrelated reason
    /// (composer keystrokes, presence ticks). Closures are not compared — they
    /// stay valid for the open chat.
    static func == (lhs: RowView, rhs: RowView) -> Bool {
        let same = lhs.row == rhs.row
            && lhs.chatId == rhs.chatId
            && lhs.width == rhs.width
            && lhs.timeReveal == rhs.timeReveal
            && lhs.highlighted == rhs.highlighted
            && lhs.searchHit == rhs.searchHit
            && lhs.mentions.stamp == rhs.mentions.stamp
            && lhs.composerInset == rhs.composerInset
            && lhs.people == rhs.people
            && lhs.canReply == rhs.canReply
            && lhs.replyDrag == rhs.replyDrag
            && lhs.replyArmed == rhs.replyArmed
            && lhs.theme == rhs.theme
            && lhs.showReactionSenders == rhs.showReactionSenders
        #if DEBUG
        PerfCounters.bump(same ? "rowEqTrue" : "rowEqFalse")
        #endif
        return same
    }

    /// The widest a bubble on this row may be: the screen minus the chat kind's inset
    /// (`Theme.bubbleInset(people:)`); an incoming people-chat bubble also loses the lane.
    private var maxBubble: CGFloat { Self.maxBubble(width: width, people: people) }

    static func maxBubble(width: CGFloat, people: Bool) -> CGFloat {
        max(200, width - Theme.bubbleInset(people: people))
    }

    private var reportsGreetingFrame: Bool {
        model.firstLogin != nil && model.messages(of: chatId).first(where: { $0.greeting })?.id == row.messageId
    }

    /// Bubbles get a swipe time; centred system / card / error rows do not.
    private var showsSwipeTime: Bool {
        switch row.kind {
        case .system, .question, .error, .forwardHint, .secretGhost, .hidden:
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
                if showReactionSenders {
                    ForEach(row.reactions, id: \.emoji) { reaction in
                        reactionSenders(reaction)
                    }
                } else if let emoji = selectedReactionEmoji,
                          let reaction = row.reactions.first(where: { $0.emoji == emoji }) {
                    reactionSenders(reaction)
                }
            }
        }
        // Quote tap: brief accent wash. Search hit: a softer persistent wash so
        // keyword matches read as a slightly different colour while searching.
        .background(
            RoundedRectangle(cornerRadius: Theme.bubbleRadius + 4, style: .continuous)
                .fill(theme.accent.opacity(highlighted ? 0.14 : (searchHit ? 0.10 : 0)))
                .padding(-4)
        )
        .animation(.easeInOut(duration: 0.35), value: highlighted)
        .animation(.easeOut(duration: 0.18), value: searchHit)
        // iMessage: drag left → times; drag right → reply. Exclusive by direction.
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
            // The reply arrow slides out from behind the bubble's leading edge.
            if canReply, replyDrag > 0.5 {
                Image(systemName: "arrowshape.turn.up.left")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(replyArmed ? theme.accent : theme.muted)
                    .frame(width: BubbleSwipe.revealWidth, alignment: .leading)
                    .offset(x: replyDrag - BubbleSwipe.revealWidth)
                    .opacity(Double(min(1, replyDrag / (BubbleSwipe.revealWidth * 0.55))))
                    .allowsHitTesting(false)
                    .accessibilityHidden(true)
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(canReply ? "bubble:\(row.messageId)" : "row:\(row.id)")
    }

    @ViewBuilder
    private var bubble: some View {
        switch row.kind {
        case .user(let text, let delivery, let quote, let preview):
            // My bubble: delivered is the full style at once; only pending is dimmed; a
            // failed one is full with the red mark on its outer edge — the words and Try again
            // are the composer's note (Jacob, 2026-10-24 / 2026-10-26; docs/mac-design-rules.md).
            HStack(spacing: 8) {
                Spacer(minLength: 0)
                if delivery == .failed {
                    Button {
                        model.resendFailed(chatId: chatId, messageId: row.messageId)
                    } label: {
                        Image(systemName: "exclamationmark.circle.fill")
                            .font(.system(size: 16))
                            .foregroundStyle(theme.danger) // error-ui: exception failed-bubble-mark
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Not sent. Retry")
                }
                if delivery == .pending {
                    Image(systemName: "clock")
                        .font(.system(size: 12))
                        .foregroundStyle(theme.muted)
                        .accessibilityLabel("Sending")
                }
                let linkOnly = text.isEmpty && preview != nil
                VStack(alignment: .leading, spacing: 6) {
                    if let quote {
                        quoteHeader(quote, color: theme.userText)
                    }
                    // No `.textSelection(.enabled)`: it steals pans from the chat
                    // ScrollView so a drag on / onto a bubble sticks
                    // ([feedback app-01M5Q1N6C1HAKHC6P2XPEWMRSS]). Copy is on long-press.
                    if !linkOnly {
                        TypedBody(text: text, ink: theme.userText, chip: theme.userCodeChip, mentions: mentions)
                    }
                    // The link preview card under the text — or as the whole bubble when the
                    // message was only the link ([feedback link-embeds-ios-2026-10-27]).
                    if let preview {
                        LinkPreviewCard(preview: preview, chatId: chatId, messageId: row.messageId, own: true)
                            .padding(.top, linkOnly ? 0 : 2)
                    }
                }
                .padding(.horizontal, linkOnly ? LinkPreviewCard.bubbleInset : Theme.bubbleHPad)
                .padding(.vertical, linkOnly ? LinkPreviewCard.bubbleInset : Theme.bubbleVPad)
                .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleUser))
                .overlay(alignment: .topTrailing) { replyCountBadge(userBubble: true) }
                .frame(maxWidth: maxBubble, alignment: .trailing)
                .opacity(delivery.opacity)
                .accessibilityIdentifier("own-bubble-\(delivery)")
            }
        case .other(let text, let handle, let avatar, let named, let quote, let preview):
            // The avatar and the name are one profile target (`ProfileToggle`): a tap
            // opens the person's sheet, a tap on either while it is up dismisses it.
            let personTarget = model.personTarget(handle: handle, avatarUrl: avatar, chatId: chatId)
            let personName = model.personName(handle: handle) ?? "@\(handle)"
            let personLabel = ProfileToggle.label(name: personName, showing: model.profileShows(personTarget))
            let openPerson = { model.toggleProfile(personTarget) }
            HStack(alignment: .bottom, spacing: Theme.memberAvatarGap) {
                Button(action: openPerson) {
                    PersonAvatar(url: avatar, handle: handle, size: Theme.memberAvatar, name: model.personName(handle: handle), id: model.personId(handle: handle))
                }
                .buttonStyle(.plain)
                .padding(.leading, Theme.avatarColumnInset(composerInset: composerInset))
                .accessibilityLabel(personLabel)
                .accessibilityIdentifier("sender-avatar")
                // Group / channel: the name inside the bubble, in the sender's colour,
                // then the reply quote, then the text. No time in the bubble (drag to reveal).
                VStack(alignment: .leading, spacing: 0) {
                    if named {
                        Button(action: openPerson) {
                            LiveSenderName(name: personName,
                                           url: avatar, seed: model.personId(handle: handle) ?? handle, agent: false)
                        }
                        .buttonStyle(.plain)
                        .padding(.bottom, Self.nameToTextGap)
                        .accessibilityLabel(personLabel)
                        .accessibilityIdentifier("bubble-name")
                    }
                    if let quote {
                        quoteHeader(quote, color: theme.text)
                            .padding(.bottom, Self.quoteToTextGap)
                    }
                    if !(text.isEmpty && preview != nil) {
                        TypedBody(text: text, ink: theme.text, chip: theme.codeChip, mentions: mentions)
                    }
                    if let preview {
                        LinkPreviewCard(preview: preview, chatId: chatId, messageId: row.messageId, own: false)
                            .padding(.top, text.isEmpty ? 0 : 8)
                    }
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
            // One identity for the disc, the caption and the bar (#1592): the same resolver
            // the presence row asks, with the same row fields.
            let agent = model.agent(sender: agentId ?? "", handle: handle, name: name, chatId: chatId)
            // The disc and the name are one profile target (`ProfileToggle`): a tap opens
            // the agent's sheet, a tap on either while it is up dismisses it.
            let memberTarget: ProfileTarget = agentId.map { ProfileTarget.agent(projectId: $0) }
                ?? model.personTarget(handle: handle, avatarUrl: avatar, chatId: chatId)
            let memberLabel = ProfileToggle.label(name: agent.name, showing: model.profileShows(memberTarget))
            let openMember = { model.toggleProfile(memberTarget) }
            HStack(alignment: .bottom, spacing: Theme.memberAvatarGap) {
                Button(action: openMember) { memberAvatar(agent) }
                    .padding(.leading, Theme.avatarColumnInset(composerInset: composerInset))
                    .buttonStyle(.plain)
                    .accessibilityLabel(memberLabel)
                    .accessibilityIdentifier("sender-avatar")
                memberBubble(
                    agent,
                    content: VStack(alignment: .leading, spacing: 0) {
                        if named {
                            Button(action: openMember) {
                                memberName(agent)
                            }
                            .buttonStyle(.plain)
                            .padding(.bottom, Self.nameToTextGap)
                            .accessibilityLabel(memberLabel)
                            .accessibilityIdentifier("bubble-name")
                        }
                        if let quote {
                            quoteHeader(quote, color: theme.text)
                                .padding(.bottom, Self.quoteToTextGap)
                        }
                        VStack(alignment: .leading, spacing: 8) {
                            ForEach(Array(blocks.enumerated()), id: \.offset) { i, block in
                                BlockView(block: block, live: live && i == blocks.count - 1, mentions: mentions, chatId: chatId, messageId: row.messageId)
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
                        BlockView(block: block, live: live && i == blocks.count - 1, mentions: mentions, chatId: chatId, messageId: row.messageId)
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
        case .hidden(let count, _, let handle, let name):
            // The blocker's folded line ([feedback blocked-state-ios-2026-10-26]); Unblock
            // opens the person's sheet (its Unblock asks first).
            let who = (name?.trimmingCharacters(in: .whitespaces)).flatMap { $0.isEmpty ? nil : $0 } ?? "@\(handle)"
            HStack(spacing: 5) {
                Image(systemName: "nosign").font(.system(size: 12, weight: .regular))
                Text(BlockedState.hiddenLine(count: count, who: who))
                Text("·")
                Button(BlockedState.unblock) {
                    model.openPersonProfile(handle: handle, avatarUrl: nil, chatId: chatId)
                }
                .buttonStyle(.plain)
                .foregroundStyle(theme.accent)
                .fontWeight(.semibold)
                .accessibilityIdentifier("hidden-row-unblock")
            }
            .font(theme.font(Theme.small))
            .foregroundStyle(theme.muted)
            .frame(maxWidth: .infinity)
            .multilineTextAlignment(.center)
            .accessibilityIdentifier("hidden-row")
        case .secretGhost(let name, let mine, let handle, let avatar, let named):
            // The ghost: surface fill with a hairline, muted ink, the key glyph; the name is
            // the only thing in text ink. Mine sits right like my bubble, another member's
            // left under their name with their avatar — never a value, never selectable.
            let lead = mine ? "Secret saved as " : "\(model.personName(handle: handle) ?? "@\(handle)") saved a secret as "
            let ghost = HStack(spacing: 7) {
                Image(systemName: "key")
                    .font(.system(size: 14, weight: .medium))
                    .foregroundStyle(theme.muted)
                (Text(lead).foregroundColor(theme.muted) + Text(name).foregroundColor(theme.text).fontWeight(.medium))
                    .font(theme.font(Theme.body))
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, Theme.bubbleHPad)
            .padding(.vertical, Theme.bubbleVPad)
            .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.card))
            .overlay(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).strokeBorder(theme.separator, lineWidth: 1))
            .accessibilityLabel("\(lead)\(name)")
            .accessibilityIdentifier("secret-ghost")
            if mine {
                HStack(spacing: 8) {
                    Spacer(minLength: 0)
                    ghost.frame(maxWidth: maxBubble, alignment: .trailing)
                }
            } else {
                HStack(alignment: .bottom, spacing: Theme.memberAvatarGap) {
                    PersonAvatar(url: avatar, handle: handle, size: Theme.memberAvatar, name: model.personName(handle: handle), id: model.personId(handle: handle))
                        .padding(.leading, Theme.avatarColumnInset(composerInset: composerInset))
                    ghost.frame(maxWidth: maxBubble - Theme.memberLane(composerInset: composerInset), alignment: .leading)
                    Spacer(minLength: 0)
                }
            }
        case .question(let questionId, let text, let options, let multi, let picked, let open, let lane, let shellCard, let offer):
            // The card is a transcript row like a bubble: the same left edge (after the avatar
            // lane in a people chat) and the same cap width as an incoming bubble.
            HStack(spacing: 0) {
                if lane { Color.clear.frame(width: Theme.memberLane(composerInset: composerInset), height: 1) }
                QuestionCard(rowId: row.id, chatId: chatId, questionId: questionId, title: text, options: options,
                             multi: multi, picked: picked, open: open, shellCard: shellCard, offer: offer)
                    .frame(maxWidth: lane ? maxBubble - Theme.memberLane(composerInset: composerInset) : maxBubble, alignment: .leading)
                Spacer(minLength: 0)
            }
        case .file(let item, let subtitle, let caption, let forwardedFrom, let sender, let named):
            if let sender {
                mediaRow(sender: sender, named: named) {
                    FileCard(item: item, subtitle: subtitle, chatId: chatId, fromUser: false, caption: caption,
                             forwardedFrom: forwardedFrom)
                        .overlay(alignment: .topTrailing) { replyCountBadge(userBubble: false) }
                        .frame(maxWidth: maxBubble - Theme.memberLane(composerInset: composerInset), alignment: .leading)
                }
            } else {
                HStack {
                    if row.role == .user { Spacer(minLength: 0) }
                    FileCard(item: item, subtitle: subtitle, chatId: chatId, fromUser: row.role == .user, caption: caption,
                             forwardedFrom: forwardedFrom)
                        .overlay(alignment: .topTrailing) { replyCountBadge(userBubble: row.role == .user) }
                        .frame(maxWidth: maxBubble, alignment: row.role == .user ? .trailing : .leading)
                    if row.role != .user { Spacer(minLength: 0) }
                }
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
        case .image(let item, let fromUser, let caption, let lane, let source, let forwardedFrom, let sender, let named):
            // Same left edge and cap as an incoming bubble: after the avatar lane in a
            // people chat ([feedback ADUIX9xlGQxCSzeXke7JVBw], Mac Attachment.lane) — with
            // the sender's avatar in that lane and the name above the first card.
            if let sender {
                mediaRow(sender: sender, named: named) {
                    ImageBubble(item: item, chatId: chatId, fromUser: false,
                                maxWidth: maxBubble - Theme.memberLane(composerInset: composerInset),
                                caption: caption, mentions: mentions, source: source, forwardedFrom: forwardedFrom)
                        .overlay(alignment: .topTrailing) { replyCountBadge(userBubble: false) }
                }
            } else {
                HStack(spacing: 0) {
                    if fromUser { Spacer(minLength: 0) }
                    if lane { Color.clear.frame(width: Theme.memberLane(composerInset: composerInset), height: 1) }
                    ImageBubble(item: item, chatId: chatId, fromUser: fromUser,
                                maxWidth: lane ? maxBubble - Theme.memberLane(composerInset: composerInset) : maxBubble,
                                caption: caption, mentions: mentions, source: source, forwardedFrom: forwardedFrom)
                        .overlay(alignment: .topTrailing) { replyCountBadge(userBubble: fromUser) }
                    if !fromUser { Spacer(minLength: 0) }
                }
            }
        case .error(let code, let message):
            // An agent turn that failed: the shared element in the transcript's flow (the agent's
            // own sentence, the code behind Details, Try again re-runs the turn).
            ErrorNote(id: "agent-turn-error",
                      error: UserError(verb: "", class: message.trimmingCharacters(in: .whitespaces).isEmpty ? .unknown : .refused(message: message),
                                       raw: code),
                      detailsOpen: $errorDetailsOpen,
                      onTryAgain: { model.retry(chatId: chatId, messageId: row.messageId) })
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

    /// Compact chips under the bubble (iMessage-style). Tap toggles my reaction
    /// with that emoji; long-press lists everyone who used it below the chips.
    private var reactionChips: some View {
        let mine = model.myUserId
        return HStack(spacing: 4) {
            if row.role == .user { Spacer(minLength: 0) }
            ForEach(row.reactions, id: \.emoji) { reaction in
                let mineOn = mine.map { reaction.senders.contains($0) } ?? false
                let selected = !showReactionSenders && selectedReactionEmoji == reaction.emoji
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
                        .fill((mineOn || selected) ? theme.accent.opacity(0.22) : theme.card)
                )
                .overlay(
                    Capsule(style: .continuous)
                        .strokeBorder(
                            (mineOn || selected) ? theme.accent.opacity(0.55) : theme.separator,
                            lineWidth: 1
                        )
                )
                .contentShape(Capsule(style: .continuous))
                .onTapGesture {
                    model.toggleReaction(chatId: chatId, messageId: row.messageId, emoji: reaction.emoji)
                }
                // High-priority so the row's bubble long-press (capsule + menu)
                // does not also fire when holding a chip.
                .highPriorityGesture(
                    LongPressGesture(minimumDuration: 0.35)
                        .onEnded { _ in
                            guard !showReactionSenders else { return }
                            selectedReactionEmoji = selected ? nil : reaction.emoji
                        }
                )
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("\(reaction.emoji) reaction, \(reaction.senders.count)")
                .accessibilityHint(mineOn
                    ? "Removes your reaction. Long-press to see who used it."
                    : "Adds your reaction. Long-press to see who used it.")
                .accessibilityAddTraits(.isButton)
                .accessibilityAction(named: "See who reacted") {
                    guard !showReactionSenders else { return }
                    selectedReactionEmoji = selected ? nil : reaction.emoji
                }
            }
            if row.role != .user { Spacer(minLength: 0) }
        }
        .padding(.leading, reactionLeading)
    }

    /// Names of everyone who used `reaction`'s emoji, under the chips.
    private func reactionSenders(_ reaction: MessageReaction) -> some View {
        let names = reaction.senders.map(reactionSenderLabel).joined(separator: ", ")
        return HStack(spacing: 0) {
            if row.role == .user { Spacer(minLength: 0) }
            Text(names)
                .font(theme.font(12))
                .foregroundStyle(theme.muted)
                .multilineTextAlignment(row.role == .user ? .trailing : .leading)
                .fixedSize(horizontal: false, vertical: true)
            if row.role != .user { Spacer(minLength: 0) }
        }
        .padding(.leading, reactionLeading)
        .accessibilityLabel("\(reaction.emoji) from \(names)")
    }

    /// Display name for a reaction sender id (`user:…` / `agent:…`).
    private func reactionSenderLabel(_ sender: String) -> String {
        if sender == model.myUserId { return "You" }
        if let member = model.peopleChat(chatId)?.members.first(where: { $0.member == sender }) {
            return member.displayName
        }
        if sender.hasPrefix("agent:"),
           let id = sender.split(separator: ":", maxSplits: 1).last.map(String.init) {
            if let project = model.project(id) ?? model.foreignAgent(id) {
                return project.name
            }
        }
        if sender.hasPrefix("user:"),
           let id = sender.split(separator: ":", maxSplits: 1).last.map(String.init) {
            if let me = model.user, me.id == id { return "You" }
            if let person = model.directory.first(where: { $0.id == id }) {
                return person.displayName
            }
        }
        if let bare = sender.split(separator: ":", maxSplits: 1).last.map(String.init), !bare.isEmpty {
            return bare
        }
        return sender
    }

    /// Match the avatar offset used by people-chat bubbles (26 pt + 8 pt gap).
    private var reactionLeading: CGFloat {
        switch row.kind {
        case .other, .member: return Theme.memberLane(composerInset: composerInset)
        case .question(_, _, _, _, _, _, let lane, _, _): return lane ? Theme.memberLane(composerInset: composerInset) : 0
        case .secretGhost(_, let mine, _, _, _): return mine ? 0 : Theme.memberLane(composerInset: composerInset)
        case .image(_, _, _, let lane, _, _, _, _): return lane ? Theme.memberLane(composerInset: composerInset) : 0
        default: return 0
        }
    }

    /// The disc beside a people-chat agent's bubble: the agent's picture through the shared
    /// identity (`AppModel.agent`) — its uploaded picture, its leaf, never initials seeded by
    /// the handle (#1592).
    private func memberAvatar(_ agent: Project) -> some View {
        AgentAvatar(project: agent, theme: theme, size: Theme.memberAvatar, still: true)
    }

    /// Someone else's picture / file row in a people chat: their avatar on the lane (the
    /// same profile target as their bubble's), the name above the card when `named`, then
    /// the card — a picture is a message like any other
    /// ([feedback forward-attachments-ios-2026-10-27], Jacob's Scout case).
    @ViewBuilder
    private func mediaRow<Card: View>(sender: MediaSender, named: Bool, @ViewBuilder card: () -> Card) -> some View {
        switch sender {
        case .person(let handle, let avatar):
            let personTarget = model.personTarget(handle: handle, avatarUrl: avatar, chatId: chatId)
            let personName = model.personName(handle: handle) ?? "@\(handle)"
            let personLabel = ProfileToggle.label(name: personName, showing: model.profileShows(personTarget))
            let openPerson = { model.toggleProfile(personTarget) }
            HStack(alignment: .bottom, spacing: Theme.memberAvatarGap) {
                Button(action: openPerson) {
                    PersonAvatar(url: avatar, handle: handle, size: Theme.memberAvatar, name: model.personName(handle: handle), id: model.personId(handle: handle))
                }
                .buttonStyle(.plain)
                .padding(.leading, Theme.avatarColumnInset(composerInset: composerInset))
                .accessibilityLabel(personLabel)
                .accessibilityIdentifier("sender-avatar")
                VStack(alignment: .leading, spacing: 4) {
                    if named {
                        Button(action: openPerson) {
                            LiveSenderName(name: personName, url: avatar, seed: model.personId(handle: handle) ?? handle, agent: false)
                        }
                        .buttonStyle(.plain)
                        .padding(.leading, 6)
                        .accessibilityLabel(personLabel)
                        .accessibilityIdentifier("bubble-name")
                    }
                    card()
                }
                Spacer(minLength: 0)
            }
            .task(id: avatar) { SenderAccents.shared.load(url: avatar) }
        case .agent(let id, let handle, let name, let avatar):
            let agent = model.agent(sender: id ?? "", handle: handle, name: name, chatId: chatId)
            let memberTarget: ProfileTarget = id.map { ProfileTarget.agent(projectId: $0) }
                ?? model.personTarget(handle: handle, avatarUrl: avatar, chatId: chatId)
            let memberLabel = ProfileToggle.label(name: agent.name, showing: model.profileShows(memberTarget))
            let openMember = { model.toggleProfile(memberTarget) }
            HStack(alignment: .bottom, spacing: Theme.memberAvatarGap) {
                Button(action: openMember) { memberAvatar(agent) }
                    .padding(.leading, Theme.avatarColumnInset(composerInset: composerInset))
                    .buttonStyle(.plain)
                    .accessibilityLabel(memberLabel)
                    .accessibilityIdentifier("sender-avatar")
                VStack(alignment: .leading, spacing: 4) {
                    if named {
                        Button(action: openMember) { memberName(agent) }
                            .buttonStyle(.plain)
                            .padding(.leading, 6)
                            .accessibilityLabel(memberLabel)
                            .accessibilityIdentifier("bubble-name")
                    }
                    card()
                }
                Spacer(minLength: 0)
            }
        }
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
            ForwardedHeader(by: by, color: color)
        } else {
            replyQuoteHeader(quote, color: color)
        }
    }

    private func replyQuoteHeader(_ quote: Quote, color: Color) -> some View {
        // Reply in DM (#1836): the quote names its chat; it is a link only when the viewer
        // can open that chat (a member), else just the words.
        let linkable = quote.source.map { model.isMember(ofChat: $0.chatId) } ?? true
        return Button {
            guard linkable else { return }
            onQuoteTap?(quote)
        } label: {
            VStack(alignment: .leading, spacing: 2) {
                if let src = quote.source {
                    Text(src.label)
                        .font(theme.font(Theme.tiny))
                        .lineLimit(1)
                        .truncationMode(.tail)
                        .accessibilityIdentifier("reply-quote-source")
                }
                HStack(spacing: 6) {
                    Image(systemName: "bubble.left")
                        .font(.system(size: 11, weight: .medium))
                    Text("“\(quote.text)”")
                        .font(theme.font(Theme.small))
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
            }
            .foregroundStyle(color.opacity(0.72))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!linkable)
        .accessibilityIdentifier("reply-quote")
        .accessibilityLabel(quote.source.map { "\($0.label): \(quote.text)" } ?? "Replying to: \(quote.text)")
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
    /// Site icons for the block's named links: the block redraws when one arrives.
    @ObservedObject private var linkIcons = LinkIcons.shared
    /// Spoilers the viewer tapped open: the block redraws to show them.
    @ObservedObject private var spoilers = SpoilerState.shared
    let block: Block
    let live: Bool
    /// Shared mention table from the row (built once per directory sync).
    let mentions: MentionTable
    /// The row's chat and message, for blocks that act on the message (a link card's
    /// Remove preview) — nil in previews that have none.
    var chatId: Id = ""
    var messageId: Id = ""
    /// The bubble's ink: the theme's text on an agent's bubble, `userText` on my own
    /// (`nil` = `theme.text`). Quotes draw their blocks in a muted cut of it.
    var ink: Color? = nil
    /// The code chip behind `` `code` `` (`nil` = `theme.codeChip`).
    var chip: Color? = nil

    /// The quote bar's width and the gap to the quoted text (Mac `QUOTE_BAR` / `QUOTE_GAP`).
    static let quoteBar: CGFloat = 3
    static let quoteGap: CGFloat = 10

    private var color: Color { ink ?? theme.text }
    private var codeChip: Color { chip ?? theme.codeChip }
    /// The muted ink: the theme's muted on the grey bubble, the ink at lower alpha on mine.
    private var muted: Color { ink == nil ? theme.muted : color.opacity(0.72) }

    var body: some View {
        #if DEBUG
        let _ = PerfCounters.bump("blockBody")
        #endif
        switch block {
        case .paragraph(let text):
            if live {
                StreamingText(text: text, font: .system(size: Theme.body), color: color)
                    .lineSpacing(Theme.bodyLineSpacing)
            } else {
                Markdown.inlineText(text, color: color, link: theme.accent, codeChip: codeChip, mentions: mentions)
                    .font(theme.font(Theme.body))
                    .lineSpacing(Theme.bodyLineSpacing)
                    .tint(theme.accent)
            }
        case .heading(let level, let text):
            // Bolder, larger text; never an anchor (Mac: 20 / 17 / 15 pt on a 13 pt body).
            Markdown.inlineText(text, color: color, link: theme.accent, codeChip: codeChip, mentions: mentions)
                .font(theme.font(Self.headingSize(level), weight: .bold))
                .tint(theme.accent)
                .padding(.top, 4)
                .accessibilityIdentifier("heading-\(level)")
        case .quote(let inner):
            // `> quoted`: a thin bar, the blocks inset and muted; a quote inside a quote
            // draws its own bar inside this one ([feedback chat-markdown-ios-2026-10-27]).
            HStack(alignment: .top, spacing: Self.quoteGap) {
                Capsule(style: .continuous)
                    .fill(muted.opacity(0.55))
                    .frame(width: Self.quoteBar)
                    .accessibilityIdentifier("quote-bar")
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(Array(inner.enumerated()), id: \.offset) { _, b in
                        BlockView(block: b, live: false, mentions: mentions, ink: muted, chip: chip)
                    }
                }
            }
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityIdentifier("quote-block")
        case .bullets(let items), .numbered(let items):
            let numbered: Bool = { if case .numbered = block { return true } else { return false } }()
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(items.enumerated()), id: \.offset) { i, item in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(numbered ? "\(i + 1)." : "•")
                            .font(theme.font(Theme.body))
                            .foregroundStyle(muted)
                            .frame(minWidth: 16, alignment: .trailing)
                        // HStack otherwise gives the text a single-line width
                        // proposal and it truncates with "…"; grow vertically.
                        if live && i == items.count - 1 {
                            StreamingText(text: item, font: .system(size: Theme.body), color: color)
                                .fixedSize(horizontal: false, vertical: true)
                        } else {
                            Markdown.inlineText(item, color: color, link: theme.accent, codeChip: codeChip, mentions: mentions)
                                .font(theme.font(Theme.body))
                                .tint(theme.accent)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }
        case .table(let header, let rows):
            TableBlock(header: header, rows: rows, mentions: mentions)
        case .code(let lang, let code):
            CodeCard(lang: lang, code: code)
        case .chart(let partId, let spec, _):
            ChartCard(partId: partId, spec: spec)
        case .linkPreview(_, let preview):
            LinkPreviewCard(preview: preview, chatId: chatId, messageId: messageId, own: false)
        case .math(let source):
            // Display math: typeset, a size up, centred on its own line (KaTeX's display
            // mode in Grok Bot).
            Text(Markdown.math(source, color: color, size: Theme.body * Markdown.displayMathScale))
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(.vertical, 4)
                .accessibilityIdentifier("display-math")
        }
    }

    /// Heading sizes against the 17 pt body: the Mac's 20 / 17 / 15 on 13, scaled.
    static func headingSize(_ level: Int) -> CGFloat {
        switch level {
        case 1: return 24
        case 2: return 21
        default: return 19
        }
    }
}

/// A typed message's body — my bubble, or another person's — through the same blocks an
/// agent's bubble draws: quotes, headings, lists, code cards and the inline styles. A line
/// with no formatting is still one `Text` ([feedback chat-markdown-ios-2026-10-27]).
struct TypedBody: View {
    let text: String
    let ink: Color
    let chip: Color
    let mentions: MentionTable

    var body: some View {
        let blocks = Markdown.blocks(text)
        VStack(alignment: .leading, spacing: 6) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                BlockView(block: block, live: false, mentions: mentions, ink: ink, chip: chip)
            }
        }
    }
}

/// True while a nested horizontal `ScrollView` (markdown table) is tracking a
/// pan, so `ChatView`'s time-reveal drag does not also slide the chat
/// ([feedback ACQD4mUCTPxH7z3Y7YO8Vjw]).
enum NestedHorizontalScroll {
    static var claiming = false
}

/// Finds the enclosing horizontal `UIScrollView` and, while its pan is active,
/// cancels non-scroll pan recognizers (SwiftUI's time-reveal `DragGesture`) so
/// the table can scroll sideways without sliding the transcript.
private struct PreferHorizontalScroll: UIViewRepresentable {
    func makeUIView(context: Context) -> Probe {
        let probe = Probe()
        probe.isUserInteractionEnabled = false
        probe.backgroundColor = .clear
        return probe
    }

    func updateUIView(_ uiView: Probe, context: Context) {
        DispatchQueue.main.async { uiView.attachIfNeeded() }
    }

    final class Probe: UIView {
        private weak var scroll: UIScrollView?
        private var observing = false

        override func didMoveToWindow() {
            super.didMoveToWindow()
            attachIfNeeded()
        }

        deinit {
            scroll?.panGestureRecognizer.removeTarget(self, action: #selector(pan(_:)))
            if NestedHorizontalScroll.claiming { NestedHorizontalScroll.claiming = false }
        }

        func attachIfNeeded() {
            guard window != nil, !observing else { return }
            guard let found = Self.enclosingScrollView(from: self) else { return }
            scroll = found
            found.panGestureRecognizer.addTarget(self, action: #selector(pan(_:)))
            observing = true
        }

        @objc private func pan(_ gr: UIPanGestureRecognizer) {
            switch gr.state {
            case .began, .changed:
                // Only claim when the table is actually wider than its frame;
                // a fitting table must still allow iMessage time reveal.
                guard let scroll, scroll.contentSize.width > scroll.bounds.width + 1 else {
                    NestedHorizontalScroll.claiming = false
                    return
                }
                NestedHorizontalScroll.claiming = true
                Self.cancelNonScrollPans(from: window, except: gr)
            case .ended, .cancelled, .failed:
                NestedHorizontalScroll.claiming = false
            default:
                break
            }
        }

        private static func enclosingScrollView(from view: UIView) -> UIScrollView? {
            var node: UIView? = view.superview
            while let current = node {
                if let scroll = current as? UIScrollView { return scroll }
                node = current.superview
            }
            return nil
        }

        /// Drop SwiftUI pan recognizers (time reveal) mid-gesture; leave every
        /// `UIScrollView` pan alone so the table and the transcript still scroll.
        private static func cancelNonScrollPans(from root: UIView?, except keep: UIGestureRecognizer) {
            guard let root else { return }
            for gr in root.gestureRecognizers ?? [] where gr !== keep {
                guard gr is UIPanGestureRecognizer, !(gr.view is UIScrollView) else { continue }
                if gr.state == .possible || gr.state == .began || gr.state == .changed {
                    gr.isEnabled = false
                    gr.isEnabled = true
                }
            }
            for sub in root.subviews {
                cancelNonScrollPans(from: sub, except: keep)
            }
        }
    }
}

/// A GFM table as a real table (the Mac's `render_block` table): a header row on
/// the code wash, body rows with hairlines between them, one column per header
/// cell with the same width in every row, each cell inline markdown — so a
/// `[Name](url)` is a tappable link, never raw markup ([feedback
/// markdown-tables-ios-2026-10-25]: Jacob's Lima bookstore table came out as a
/// monospace block of `[…](https://…)` cut off at the bubble's edge). Wider
/// than the bubble, the table keeps its columns and scrolls sideways.
struct TableBlock: View {
    @Environment(\.theme) private var theme
    /// The cells' link icons: the table redraws when one arrives (its inputs alone do not
    /// change, so SwiftUI would otherwise keep the placeholders).
    @ObservedObject private var linkIcons = LinkIcons.shared
    let header: [String]
    let rows: [[String]]
    let mentions: MentionTable

    /// The narrowest a column is laid out at, in points.
    static let columnMin: CGFloat = 72
    /// The widest a column is laid out at before its cells wrap.
    static let columnMax: CGFloat = 220
    /// Average glyph advance of the 14 pt cell text, used to size columns before layout.
    static let glyphAdvance: CGFloat = 7.4

    /// One width per column, from the longest rendered cell (markup removed) in that
    /// column, clamped to `columnMin…columnMax`; every row uses the same widths, so the
    /// columns line up, and their sum is what the frame scrolls when the bubble is narrower.
    static func columnWidths(header: [String], rows: [[String]]) -> [CGFloat] {
        let cols = max(header.count, 1)
        return (0..<cols).map { ci in
            let longest = ([header] + rows)
                .compactMap { ci < $0.count ? $0[ci] : nil }
                .map { Markdown.plain($0).count }
                .max() ?? 0
            return min(max(CGFloat(longest) * glyphAdvance + 20, columnMin), columnMax)
        }
    }

    /// `row` padded with empty cells or cut to the header's width.
    static func fitted(_ row: [String], to cols: Int) -> [String] {
        Array((row + Array(repeating: "", count: max(0, cols - row.count))).prefix(cols))
    }

    var body: some View {
        let cols = max(header.count, 1)
        let widths = Self.columnWidths(header: header, rows: rows)
        ScrollView(.horizontal, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                tableRow(Self.fitted(header, to: cols), widths: widths, head: true)
                    .background(theme.codeChip)
                ForEach(Array(rows.enumerated()), id: \.offset) { ri, row in
                    tableRow(Self.fitted(row, to: cols), widths: widths, head: false)
                        .background(ri % 2 == 1 ? theme.codeChip.opacity(0.4) : .clear)
                        .overlay(alignment: .top) { Rectangle().fill(theme.separator).frame(height: 0.5) }
                }
            }
            .accessibilityIdentifier("table")
            // Inside the scroll content so the probe's superview walk reaches the
            // table's UIScrollView ([feedback ACQD4mUCTPxH7z3Y7YO8Vjw]).
            .background(PreferHorizontalScroll())
        }
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(theme.bg.opacity(0.6)))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(theme.separator, lineWidth: 0.5))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private func tableRow(_ cells: [String], widths: [CGFloat], head: Bool) -> some View {
        HStack(alignment: .top, spacing: 0) {
            ForEach(Array(cells.enumerated()), id: \.offset) { ci, cell in
                Markdown.inlineText(cell, color: head ? theme.text : theme.muted, link: theme.accent, codeChip: theme.codeChip, mentions: mentions)
                    .font(theme.font(14, weight: head ? .semibold : .regular))
                    .tint(theme.accent)
                    .multilineTextAlignment(.leading)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(width: widths[ci], alignment: .topLeading)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .overlay(alignment: .leading) {
                        if ci > 0 { Rectangle().fill(theme.separator).frame(width: 0.5) }
                    }
            }
        }
    }
}

/// A fenced ``` block as Grok Bot's code card ([feedback
/// code-highlighting-ios-2026-10-26]): no header row, a copy icon floating
/// top-right, the mono body coloured by the shared highlighter (keywords,
/// function names, strings, numbers, comments in italic), 1 pt indent guides,
/// and long lines scrolling sideways inside the card. Copy puts the block on the
/// pasteboard, tints while pressed and shows a check for 1.5 s.
struct CodeCard: View {
    @Environment(\.theme) private var theme
    let lang: String
    let code: String
    /// Screenshot rig (`ARBOS_RIG_CODE_COPY=pressed|copied`): the simulator
    /// cannot hold a touch through a capture, and the check lasts 1.5 s.
    private static let rig = ProcessInfo.processInfo.environment["ARBOS_RIG_CODE_COPY"]
    @State private var copied = CodeCard.rig == "copied"

    static let copiedFor: Duration = .seconds(1.5)
    static let radius: CGFloat = 12

    /// The button's action: the block, verbatim, on the general pasteboard.
    static func copyToPasteboard(_ code: String) {
        UIPasteboard.general.string = code
    }

    var body: some View {
        let size = CodeHighlight.fontSize * theme.textScale
        let advance = CodeHighlight.glyphAdvance(size: size)
        let lineHeight = CodeHighlight.lineHeight(size: size)
        let font = theme.font(CodeHighlight.fontSize, design: .monospaced)
        let text = CodeHighlight.attributed(lang: lang, code: code, palette: theme.codePalette, font: font, plain: theme.text)
        let guides = CodeHighlight.indentGuides(code, advance: advance)
        ZStack(alignment: .topTrailing) {
            ScrollView(.horizontal, showsIndicators: false) {
                Text(text)
                    .lineSpacing(0)
                    .fixedSize(horizontal: true, vertical: false)
                    .overlay(alignment: .topLeading) {
                        ForEach(Array(guides.enumerated()), id: \.offset) { _, guide in
                            Rectangle()
                                .fill(theme.codePalette.guide)
                                .frame(width: 1, height: lineHeight)
                                .offset(x: guide.x, y: CGFloat(guide.line) * lineHeight)
                        }
                    }
                    .padding(.leading, CodeHighlight.padX)
                    .padding(.trailing, CodeHighlight.padRight)
                    .padding(.vertical, CodeHighlight.padY)
                    .accessibilityIdentifier("code-body")
            }
            Button {
                Self.copyToPasteboard(code)
                Haptics.pick()
                copied = true
                Task { @MainActor in
                    try? await Task.sleep(for: Self.copiedFor)
                    copied = false
                }
            } label: {
                Image(systemName: copied ? "checkmark" : "doc.on.doc")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(copied ? theme.online : theme.muted)
                    .frame(width: CodeHighlight.copySize, height: CodeHighlight.copySize)
                    .background(
                        // The card's own colour, so the square covers the code under it.
                        ZStack { theme.bubbleAssistant; theme.bg.opacity(0.6) }
                            .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
                    )
                    .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).stroke(theme.separator, lineWidth: 0.5))
                    .contentShape(Rectangle())
            }
            .buttonStyle(CopyPillPress(tint: theme.accent, forced: CodeCard.rig == "pressed"))
            .accessibilityLabel(copied ? "Copied" : "Copy code")
            .accessibilityIdentifier(copied ? "code-copy-check" : "code-copy")
            .padding(CodeHighlight.copyInset)
        }
        .background(RoundedRectangle(cornerRadius: Self.radius, style: .continuous).fill(theme.bg.opacity(0.6)))
        .overlay(RoundedRectangle(cornerRadius: Self.radius, style: .continuous).stroke(theme.separator, lineWidth: 0.5))
        .clipShape(RoundedRectangle(cornerRadius: Self.radius, style: .continuous))
        .accessibilityIdentifier("code-card")
    }
}

/// The Copy pill while the finger is down: the chips' pressed tint (Mac
/// `mention::PILL_ALPHA_PRESSED`, accent at 30 %) and the label in the accent.
struct CopyPillPress: ButtonStyle {
    let tint: Color
    var forced = false

    func makeBody(configuration: Configuration) -> some View {
        let pressed = configuration.isPressed || forced
        configuration.label
            .background(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(pressed ? tint.opacity(0.30) : .clear)
            )
            .foregroundStyle(pressed ? tint : Color.primary)
            .scaleEffect(pressed ? 0.96 : 1.0)
            .animation(.easeOut(duration: 0.12), value: pressed)
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

/// Where one of my own bubbles is on its way. No caption says it (Jacob, 2026-10-24:
/// "remove these 'sent' items, just use coloring"), and nothing after delivery dims it
/// (Jacob, 2026-10-25: delivered bubbles that waited for a read receipt "show up too dim,
/// I think they have not been sent"). Same values as the Mac (`transcript.rs` `Delivery`,
/// docs/mac-design-rules.md § Own-bubble delivery).
enum Delivery: Equatable {
    /// Not yet acknowledged by the relay: the only dimmed state.
    case pending
    /// The relay refused it: full opacity plus a red "! Retry".
    case failed
    /// Acknowledged by the relay: the full own-bubble style, at once. Read receipts never
    /// change how my bubble looks — agents send none, members on older builds send none, a
    /// member who has not opened the chat sends none, and a bubble waiting for one reads as unsent.
    case delivered

    /// Whole-bubble alpha of a pending bubble (3.4 : 1 dark / 3.6 : 1 light text contrast,
    /// transient until the relay's ACK).
    static let pendingAlpha: CGFloat = 0.5

    var opacity: CGFloat {
        switch self {
        case .pending: return Delivery.pendingAlpha
        case .failed, .delivered: return 1
        }
    }

    static func of(_ message: Message) -> Delivery {
        if message.pending { return .pending }
        if message.status == .error { return .failed }
        return .delivered
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

    /// Pictures are kept at the largest size a row draws (128 pt hero at 3x). The relay's
    /// picture routes are asked for exactly this width (`?w=`), so the download is the 384 px
    /// Lanczos copy of the 1024 px master, not the master itself.
    nonisolated static let maxPixelSize = 384

    /// The relay's picture routes — `/avatars/{id}.jpg`, `/projects/{id}/avatar.jpg`,
    /// `/blob/{slot}/{key}` — take `?w=<px>` and answer with that bucket of the master
    /// (`avatar_image` on the relay). Other hosts' pictures (GitHub) are fetched as given.
    nonisolated static func sizedURL(_ url: String, width: Int) -> URL? {
        guard var comps = URLComponents(string: url) else { return URL(string: url) }
        let path = comps.path
        let relayPicture = path.contains("/avatars/") || path.hasSuffix("/avatar.jpg") || path.contains("/blob/")
        guard relayPicture else { return comps.url }
        var items = comps.queryItems ?? []
        items.removeAll { $0.name == "w" }
        items.append(URLQueryItem(name: "w", value: String(width)))
        comps.queryItems = items
        return comps.url
    }
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

    /// Waits between attempts. A cold launch asks for pictures before the network is up (a
    /// phone that just woke, the relay mid-restart); one failed attempt used to leave the leaf
    /// placeholder on every surface until the view was rebuilt (Jacob, 2026-10-26, Mac and
    /// iPhone alike).
    nonisolated static let retrySeconds: [UInt64] = [2, 5, 10, 20, 40]

    /// Shared download per URL; waiters resume together when the picture lands. Picture URLs
    /// name their version (`…/avatar.jpg?v=<stamp>`, `/avatars/<user>.jpg?v=…`), so the disk
    /// cache is asked first: a relaunch paints the picture with no network at all, and a
    /// changed picture is a new URL. Failures retry with backoff before giving up; a 404 ("no
    /// picture") does not retry.
    func load(_ url: String) async -> UIImage? {
        if let img = images[url] { return img }
        if requested.contains(url) {
            return await withCheckedContinuation { cont in
                waiters[url, default: []].append(cont)
            }
        }
        guard url.contains("://"), let u = AvatarImageCache.sizedURL(url, width: AvatarImageCache.maxPixelSize) else { return nil }
        requested.insert(url)
        var img: UIImage?
        var attempt = 0
        while true {
            let (data, status) = await Task.detached(priority: .userInitiated) { () -> (Data?, Int) in
                var req = URLRequest(url: u)
                req.cachePolicy = .returnCacheDataElseLoad
                guard let (data, resp) = try? await URLSession.shared.data(for: req) else { return (nil, 0) }
                let code = (resp as? HTTPURLResponse)?.statusCode ?? 200
                return (code / 100 == 2 ? data : nil, code)
            }.value
            if let data, let decoded = AvatarImageCache.decode(data) {
                img = decoded
                break
            }
            if status == 404 || attempt >= AvatarImageCache.retrySeconds.count { break }
            try? await Task.sleep(nanoseconds: AvatarImageCache.retrySeconds[attempt] * 1_000_000_000)
            attempt += 1
        }
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
    /// Jacob, 2026-10-25: a group created with no leaf shows its title's initial on grey.
    var neutralName: String? = nil

    private var shown: [ChatMember] { Array(members.prefix(maxVisible)) }
    private var hasMore: Bool { members.count > maxVisible }

    var body: some View {
        if let neutralName, imageUrl?.isEmpty ?? true {
            NeutralAvatar(name: neutralName, size: size * 1.12)
        } else if let leaf, imageUrl?.isEmpty ?? true, LeafAvatarsFlag.shared.on {
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
            GlassCircle(size: size) { // control-size: exception rowIconTiles (avatar cluster placeholder)
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
/// When the send carried a description, the caption sits under the picture in
/// the same rounded bubble so the note is not a second message — and that bubble
/// is a normal text bubble in width: the caption wraps at the bubble cap
/// (`maxWidth`), never at the picture's own width, which for a portrait
/// screenshot is a ~150 pt sliver ([feedback image-caption-bubble-ios-2026-10-25]).
/// A picture narrower than its bubble (portrait + long caption) sits centred at
/// the top with a small inset and its own radius; one that spans the bubble is
/// flush, exactly as a picture-only bubble.
/// The forwarded bubble's header (Telegram / iMessage): forward glyph, then "Forwarded
/// from <name>", one line, not tappable. On a text bubble, a picture bubble (above the
/// picture) and a file card alike ([feedback forward-attachments-ios-2026-10-27]).
struct ForwardedHeader: View {
    @Environment(\.theme) private var theme
    let by: String
    let color: Color

    var body: some View {
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
        .accessibilityIdentifier("forwarded-header")
    }
}

struct ImageBubble: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let item: MediaItem
    let chatId: Id
    let fromUser: Bool
    let maxWidth: CGFloat
    var caption: String? = nil
    var mentions: MentionTable? = nil
    /// GIF-picker attribution (`Attachment.source`); opens `pageUrl` when tapped.
    var source: AttachmentSource? = nil
    /// "Forwarded from <name>" above the picture, inside the bubble.
    var forwardedFrom: String? = nil

    @State private var image: UIImage?
    /// First frame only — shown while the bubble is off screen so the animation pauses.
    @State private var stillFrame: UIImage?
    @State private var animate = false
    @State private var failed = false
    @State private var viewer: MediaViewerState?

    /// Mac `picture_size`: fill the bubble width unless the picture is tall.
    static let maxHeight: CGFloat = 320
    static let minWidth: CGFloat = 120
    static let placeholder = CGSize(width: 240, height: 160)
    /// A picture narrower than its bubble: this far from the top (and centred), with
    /// `pictureRadius` corners, so it reads as a photo inside the bubble, not a sliver
    /// glued to one edge.
    static let pictureInset: CGFloat = 8
    static let pictureRadius: CGFloat = Theme.bubbleRadius - pictureInset

    private var bubbleWidth: CGFloat { min(280, maxWidth) }
    private var fill: Color { fromUser ? theme.bubbleUser : theme.bubbleAssistant }
    private var captionText: String? { Self.captionText(caption) }

    /// The caption as shown: trimmed, nil when blank.
    static func captionText(_ caption: String?) -> String? {
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
        // Only the picture opens the viewer. The description bar sits outside
        // that tap so a long-press on the text still shows Edit / Reply
        // ([feedback app-01M5TK984G6NY5A66K75RR5PWJ]).
        PhotoBubbleLayout(maxWidth: maxWidth, pictureInset: Self.pictureInset, hasHeader: forwardedFrom != nil) {
            if let forwardedFrom {
                ForwardedHeader(by: forwardedFrom, color: fromUser ? theme.userText : theme.text)
                    .padding(.horizontal, Theme.bubbleHPad)
                    .padding(.top, Theme.bubbleVPad)
                    .padding(.bottom, 6)
            }
            // Tap opens the viewer. Not a Button: the transcript's reply swipe
            // owns the row pan, and a SwiftUI Button still counts a right pan
            // that starts here as a tap on lift ([feedback
            // app-01M5Z71ZQQ9G8EG5WF8SVNGM85]). onTapGesture yields once the
            // pan moves; the flag covers the case where the swipe already claimed.
            pictureTile
                .contentShape(Rectangle())
                .onTapGesture(perform: openViewer)
                .accessibilityAddTraits(.isButton)
                .accessibilityLabel("Image, tap to view")
            if captionText != nil || source != nil {
                VStack(alignment: .leading, spacing: 2) {
                    if let captionText {
                        // The same text as a text bubble: body size, body line spacing, links
                        // and mention chips (Jacob: "normal font size").
                        Markdown.inlineText(captionText, color: fromUser ? theme.userText : theme.text, link: theme.accent,
                                            codeChip: fromUser ? theme.userCodeChip : theme.codeChip, mentions: mentions)
                            .font(theme.font(Theme.body))
                            .lineSpacing(Theme.bodyLineSpacing)
                            .tint(theme.accent)
                            .multilineTextAlignment(.leading)
                            .accessibilityIdentifier("photo-caption")
                            .accessibilityLabel("Description")
                    }
                    // GIF-picker attribution (feedback app-01M5TGH18WV3QCK5KZT6XKG7FG).
                    // `SwiftUI.Link`: model `Link` is the chat connection state.
                    if let source, let url = URL(string: source.pageUrl), !source.pageUrl.isEmpty {
                        SwiftUI.Link("via \(source.provider)", destination: url)
                            .font(theme.font(11))
                            .foregroundStyle(theme.muted)
                            .accessibilityIdentifier("photo-via-source")
                    }
                }
                .padding(.horizontal, Theme.bubbleHPad)
                .padding(.vertical, Theme.bubbleVPad)
            }
        }
        .background(fill)
        .clipShape(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous))
        .accessibilityIdentifier("photo-bubble")
        .task(id: item.blobRef) { await load() }
        .onAppear { animate = stillFrame != nil }
        .onDisappear { animate = false }
        .fullScreenCover(item: $viewer) { v in
            MediaViewer(album: v.items, index: v.index, onClose: { viewer = nil })
                .environmentObject(model)
                .environment(\.theme, theme)
        }
    }

    /// The picture at its fitted size. The layout hands this tile the bubble's width:
    /// when that is the picture's own width the picture is flush (a picture-only
    /// bubble, or a caption no wider than the picture); when the bubble is wider the
    /// picture sits centred, `pictureInset` down, with `pictureRadius` corners.
    private var pictureTile: some View {
        let picture = size
        return GeometryReader { g in
            let flush = PhotoBubbleLayout.pictureIsFlush(picture: picture, bubbleWidth: g.size.width)
            ZStack {
                if let shown = (animate ? image : stillFrame) ?? image {
                    Image(uiImage: shown)
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
            .frame(width: picture.width, height: picture.height)
            .clipShape(RoundedRectangle(cornerRadius: flush ? 0 : Self.pictureRadius, style: .continuous))
            .frame(width: g.size.width, height: g.size.height, alignment: flush ? .top : .bottom)
        }
        // Unproposed (the layout measuring) → the picture's size; placed → the tile the
        // layout hands it.
        .frame(minWidth: picture.width, idealWidth: picture.width, maxWidth: .infinity,
               minHeight: picture.height, idealHeight: picture.height, maxHeight: .infinity)
    }

    /// Open the full-screen viewer for this picture. Skipped while a reply
    /// swipe owns the drag so a right pan on the image only replies.
    private func openViewer() {
        guard !BackSwipe.bubbleSwipeActive else { return }
        viewer = MediaOpen.state(for: item, chatId: chatId, model: model)
    }

    private func load() async {
        guard let (data, _) = await MediaFetch.bytes(for: item, model: model) else {
            failed = true
            return
        }
        // Animated GIF / WebP: every frame, no ImageTrim (feedback
        // app-01M5TGH18WV3QCK5KZT6XKG7FG). Still pictures keep the trim.
        if let animated = AnimatedImageFrames.animatedImage(from: data) {
            image = animated
            stillFrame = animated.images?.first ?? animated
            animate = true
            failed = false
        } else if let img = UIImage(data: data) {
            let trimmed = ImageTrim.trimmed(img)
            image = trimmed
            stillFrame = nil
            animate = false
            failed = false
        } else {
            failed = true
        }
    }
}

/// GIF / animated WebP frames for chat bubbles and tests
/// (feedback app-01M5TGH18WV3QCK5KZT6XKG7FG). Caps match the Mac decoder.
enum AnimatedImageFrames {
    static let maxFrames = 300
    static let maxDecodedBytes = 20 * 1024 * 1024

    /// Frame list + total duration for an animated GIF/WebP, or `nil` for a
    /// still (PNG, JPEG, single-frame WebP) or when the caps force a still.
    static func frames(from data: Data) -> (images: [UIImage], duration: TimeInterval)? {
        guard looksAnimated(data) else { return nil }
        guard let src = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        let count = CGImageSourceGetCount(src)
        guard count > 1 else { return nil }

        var images: [UIImage] = []
        var total: TimeInterval = 0
        var decoded = 0
        images.reserveCapacity(min(count, maxFrames))
        for i in 0..<count {
            if images.count >= maxFrames { return nil }
            guard let cg = CGImageSourceCreateImageAtIndex(src, i, nil) else { continue }
            let bytes = cg.bytesPerRow * cg.height
            if decoded + bytes > maxDecodedBytes { return nil }
            decoded += bytes
            let delay = frameDelay(source: src, index: i)
            total += delay
            images.append(UIImage(cgImage: cg))
        }
        guard images.count > 1 else { return nil }
        return (images, max(total, 0.1))
    }

    /// Animated `UIImage`, or `nil` when `data` is still / over cap.
    static func animatedImage(from data: Data) -> UIImage? {
        guard let (images, duration) = frames(from: data) else { return nil }
        return UIImage.animatedImage(with: images, duration: duration)
    }

    private static func looksAnimated(_ data: Data) -> Bool {
        if data.count >= 6, data.starts(with: Data("GIF8".utf8)) { return true }
        if data.count >= 12 {
            let riff = data.prefix(4) == Data("RIFF".utf8)
            let webp = data.subdata(in: 8..<12) == Data("WEBP".utf8)
            if riff && webp { return true }
        }
        return false
    }

    private static func frameDelay(source: CGImageSource, index: Int) -> TimeInterval {
        let props = CGImageSourceCopyPropertiesAtIndex(source, index, nil) as? [CFString: Any]
        if let gif = props?[kCGImagePropertyGIFDictionary] as? [CFString: Any] {
            let unclamped = gif[kCGImagePropertyGIFUnclampedDelayTime] as? Double
            let clamped = gif[kCGImagePropertyGIFDelayTime] as? Double
            let d = unclamped ?? clamped ?? 0.1
            return d < 0.02 ? 0.1 : d
        }
        if let webp = props?[kCGImagePropertyWebPDictionary] as? [CFString: Any] {
            let unclamped = webp[kCGImagePropertyWebPUnclampedDelayTime] as? Double
            let clamped = webp[kCGImagePropertyWebPDelayTime] as? Double
            let d = unclamped ?? clamped ?? 0.1
            return d < 0.02 ? 0.1 : d
        }
        return 0.1
    }
}

/// The photo bubble's geometry: the picture tile on top, the caption under it, the
/// bubble as wide as the wider of the two — the caption measured at the bubble cap
/// (`maxWidth`, a text bubble's width), never at the picture's width.
/// `geometry(picture:caption:maxWidth:)` is the pure arithmetic the layout and the
/// tests share ([feedback image-caption-bubble-ios-2026-10-25]).
struct PhotoBubbleLayout: Layout {
    let maxWidth: CGFloat
    let pictureInset: CGFloat
    /// The first subview is a "Forwarded from" header above the picture
    /// ([feedback forward-attachments-ios-2026-10-27]); it widens the bubble like a
    /// caption does, so a long name over a narrow portrait floats the picture.
    var hasHeader = false

    struct Geometry: Equatable {
        /// The bubble's size; the caption is placed at `captionTop`, full bubble width.
        var size: CGSize
        /// The picture tile's height: the picture plus its top inset when it floats.
        var tileHeight: CGFloat
        /// Where the picture tile starts: under the header when there is one.
        var tileTop: CGFloat = 0
        var captionTop: CGFloat { tileTop + tileHeight }
        /// Whether the picture spans the bubble (flush, no inset, square against the caption).
        var pictureFlush: Bool
    }

    /// `picture` is the fitted picture; `caption` its wrapped size at `cap` (padding
    /// included) or nil for a picture-only bubble; `cap` is what the row offers, at
    /// most the bubble cap.
    static func geometry(picture: CGSize, caption: CGSize?, cap: CGFloat, pictureInset: CGFloat) -> Geometry {
        geometry(picture: picture, caption: caption, header: nil, cap: cap, pictureInset: pictureInset)
    }

    /// As above with an optional header (its one-line size, padding included) on top.
    static func geometry(picture: CGSize, caption: CGSize?, header: CGSize?, cap: CGFloat,
                         pictureInset: CGFloat) -> Geometry {
        let width = min(cap, max(picture.width, caption?.width ?? 0, header?.width ?? 0))
        let flush = pictureIsFlush(picture: picture, bubbleWidth: width)
        let tile = picture.height + (flush ? 0 : pictureInset)
        let top = header?.height ?? 0
        return Geometry(size: CGSize(width: width, height: top + tile + (caption?.height ?? 0)),
                        tileHeight: tile, tileTop: top, pictureFlush: flush)
    }

    /// Flush when the bubble is no wider than the picture (half a point of slack for
    /// rounding); a wider bubble floats the picture.
    static func pictureIsFlush(picture: CGSize, bubbleWidth: CGFloat) -> Bool {
        bubbleWidth <= picture.width + 0.5
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        measure(proposal: proposal, subviews: subviews).size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let g = measure(proposal: proposal, subviews: subviews)
        let (header, picture, caption) = split(subviews)
        guard let picture else { return }
        header?.place(at: bounds.origin, anchor: .topLeading,
                      proposal: ProposedViewSize(width: g.size.width, height: nil))
        picture.place(at: CGPoint(x: bounds.minX, y: bounds.minY + g.tileTop), anchor: .topLeading,
                      proposal: ProposedViewSize(width: g.size.width, height: g.tileHeight))
        caption?.place(at: CGPoint(x: bounds.minX, y: bounds.minY + g.captionTop), anchor: .topLeading,
                       proposal: ProposedViewSize(width: g.size.width, height: nil))
    }

    /// (header, picture, caption) by position: the header only when `hasHeader`.
    private func split(_ subviews: Subviews) -> (LayoutSubview?, LayoutSubview?, LayoutSubview?) {
        var ix = subviews.startIndex
        func next() -> LayoutSubview? {
            guard ix < subviews.endIndex else { return nil }
            defer { ix += 1 }
            return subviews[ix]
        }
        let header = hasHeader ? next() : nil
        let picture = next()
        return (header, picture, next())
    }

    private func measure(proposal: ProposedViewSize, subviews: Subviews) -> Geometry {
        let cap = min(maxWidth, proposal.width ?? maxWidth)
        let (headerView, pictureView, captionView) = split(subviews)
        guard let pictureView else {
            return Geometry(size: .zero, tileHeight: 0, pictureFlush: true)
        }
        let picture = pictureView.sizeThatFits(.unspecified)
        // One line, as wide as it wants up to the cap; measured first so it counts
        // toward the width the caption wraps at.
        let header = headerView.map { v -> CGSize in
            let s = v.sizeThatFits(ProposedViewSize(width: cap, height: nil))
            return CGSize(width: min(cap, s.width), height: s.height)
        }
        guard let captionView else {
            return Self.geometry(picture: picture, caption: nil, header: header, cap: cap, pictureInset: pictureInset)
        }
        // Wrap the caption at the cap to learn the bubble's width, then once more at that
        // width so the height matches what is placed.
        let wide = captionView.sizeThatFits(ProposedViewSize(width: cap, height: nil))
        let width = min(cap, max(picture.width, wide.width, header?.width ?? 0))
        let caption = captionView.sizeThatFits(ProposedViewSize(width: width, height: nil))
        return Self.geometry(picture: picture, caption: CGSize(width: min(width, caption.width), height: caption.height),
                             header: header, cap: cap, pictureInset: pictureInset)
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
    /// "Forwarded from <name>" at the top of the card.
    var forwardedFrom: String? = nil

    @State private var viewer: MediaViewerState?

    private var captionText: String? {
        guard let caption else { return nil }
        let t = caption.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }

    var body: some View {
        // Only the card header opens the viewer. A video's description bar sits
        // outside that button so long-pressing the text shows Edit / Reply
        // instead of opening the video ([feedback app-01M5TK984G6NY5A66K75RR5PWJ]).
        VStack(alignment: .leading, spacing: 0) {
            if let forwardedFrom {
                ForwardedHeader(by: forwardedFrom, color: fromUser ? theme.userText : theme.text)
                    .padding(.horizontal, Theme.bubbleHPad)
                    .padding(.top, Theme.bubbleVPad)
            }
            Button {
                viewer = MediaOpen.state(for: item, chatId: chatId, model: model)
            } label: {
                // Grok's file card (web-08, measured): 61 pt tall, 20×27 icon at 15 pt,
                // text 16 pt after the icon, 17 pt title over a 15 pt muted subtitle.
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
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(item.blobRef == nil)
            .accessibilityLabel(item.blobRef == nil ? "\(item.name), unavailable" : "\(item.name), \(subtitle), tap to open")
            if let captionText {
                Text(captionText)
                    .font(theme.font(Theme.body))
                    .foregroundStyle(fromUser ? theme.userText : theme.text)
                    .multilineTextAlignment(.leading)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, Theme.bubbleHPad)
                    .padding(.bottom, Theme.bubbleVPad)
                    .padding(.top, 2)
                    .accessibilityLabel("Description")
            }
        }
        .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(fromUser ? theme.bubbleUser : theme.bubbleAssistant))
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
