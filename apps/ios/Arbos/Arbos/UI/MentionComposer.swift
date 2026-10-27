import ArbosFFI
import SwiftUI
import UIKit

// MARK: - Composer @-mention autocomplete (#1596, Jacob, Oct 23)
//
// Typing `@` + a partial name in a people chat opens a picker above the field
// (`ChatView.mentionList`); the picked row — or an exact `@handle` followed by a
// separator — becomes ONE chip in the field: the entity's picture and name on
// a soft pill, drawn as a text attachment so the caret skips it and ⌫ removes
// it whole. The draft stays a plain string (`@handle` where the chips are);
// `ComposerMention.range` says which `@handle` spans are chips, and the send
// carries each chip's member id in the row's `mentions`. The Mac twin is
// `ui/composer.rs` (`MentionToken`).

/// `@here` (everyone in this chat) and `@agents` (all agents here) — Jacob,
/// 2026-10-24. Any member of a group or people chat may use them. The chip's
/// id is the marker; `AppModel.mentions(in:)` expands it into the chat's
/// member ids, which is what every agent's gate reads, so the agents wake.
enum CollectiveMention: String, CaseIterable, Equatable {
    case here, agents

    var handle: String { rawValue }
    /// The picker row: `@here · everyone in this chat`.
    var description: String {
        switch self {
        case .here: return "everyone in this chat"
        case .agents: return "all agents here"
        }
    }
}

/// What a chip stands for.
enum ComposerMentionTarget: Equatable {
    case member(ChatMember)
    case collective(CollectiveMention)
}

/// One committed chip: the `@handle` span in the plain draft (UTF-16 offsets)
/// and the member — or collective — it stands for.
struct ComposerMention: Equatable {
    var range: Range<Int>
    let target: ComposerMentionTarget

    init(range: Range<Int>, target: ComposerMentionTarget) {
        self.range = range
        self.target = target
    }

    init(range: Range<Int>, member: ChatMember) {
        self.init(range: range, target: .member(member))
    }

    var member: ChatMember? {
        if case .member(let m) = target { return m }
        return nil
    }
    var collective: CollectiveMention? {
        if case .collective(let c) = target { return c }
        return nil
    }
    var isCollective: Bool { collective != nil }

    /// What the text reads after `@` (an agent's name slug, a person's handle, `here`).
    var handle: String {
        switch target {
        case .member(let m): return ComposerMention.handle(of: m)
        case .collective(let c): return c.handle
        }
    }
    /// The chip's label — as the bubble chips read it (`here` / `agents` for the collectives).
    var name: String {
        switch target {
        case .member(let m):
            if m.isAgent { return (m.project?.name ?? m.handle).trimmingCharacters(in: .whitespaces) }
            return m.name?.isEmpty == false ? m.name! : m.handle
        case .collective(let c):
            return c.handle
        }
    }
    /// `user:<id>` / `agent:<project-id>`, or the collective marker the send expands.
    var memberId: String {
        switch target {
        case .member(let m): return m.member
        case .collective(let c): return c.handle
        }
    }
    /// The plain text of the chip.
    var plain: String { "@" + handle }

    static func handle(of m: ChatMember) -> String {
        m.isAgent ? mentionSlug(m.project?.name ?? m.handle) : m.handle
    }
}

/// Pure draft logic, shared by the field and the tests.
enum MentionDraft {
    /// Separators that complete a typed `@handle` (behaviour 5).
    static func completesHandle(_ c: Character) -> Bool {
        c.isWhitespace || ".,!?:;".contains(c)
    }

    /// The `@token` the caret sits at the end of — `@` at a word start, no
    /// whitespace up to the caret, not inside a chip — as (query, UTF-16 range
    /// of `@token` in `text`).
    static func openToken(text: String, caret: Int, mentions: [ComposerMention]) -> (query: String, range: Range<Int>)? {
        let ns = text as NSString
        guard caret >= 0, caret <= ns.length else { return nil }
        if mentions.contains(where: { $0.range.lowerBound < caret && caret < $0.range.upperBound }) { return nil }
        var at = caret - 1
        while at >= 0 {
            let c = ns.character(at: at)
            if c == 0x40 /* @ */ { break }
            if let scalar = Unicode.Scalar(c), CharacterSet.whitespacesAndNewlines.contains(scalar) { return nil }
            at -= 1
        }
        guard at >= 0 else { return nil }
        if at > 0 {
            let before = ns.character(at: at - 1)
            guard let scalar = Unicode.Scalar(before), CharacterSet.whitespacesAndNewlines.contains(scalar) else { return nil }
        }
        if mentions.contains(where: { $0.range.contains(at) }) { return nil }
        return (ns.substring(with: NSRange(location: at + 1, length: caret - at - 1)).lowercased(), at..<caret)
    }

    /// Replace `range` of `text` with `with`, keeping the chips right: a chip the
    /// range touches goes whole, chips after it shift. Returns the replaced range.
    @discardableResult
    static func splice(_ text: inout String, _ range: Range<Int>, with: String, mentions: inout [ComposerMention]) -> Range<Int> {
        var range = range
        for m in mentions where m.range.lowerBound < range.upperBound && m.range.upperBound > range.lowerBound {
            range = min(range.lowerBound, m.range.lowerBound)..<max(range.upperBound, m.range.upperBound)
        }
        let delta = (with as NSString).length - range.count
        mentions.removeAll { $0.range.lowerBound < range.upperBound && $0.range.upperBound > range.lowerBound }
        for i in mentions.indices where mentions[i].range.lowerBound >= range.upperBound {
            mentions[i].range = (mentions[i].range.lowerBound + delta)..<(mentions[i].range.upperBound + delta)
        }
        text = (text as NSString).replacingCharacters(in: NSRange(location: range.lowerBound, length: range.count), with: with)
        mentions.sort { $0.range.lowerBound < $1.range.lowerBound }
        return range
    }

    /// Put `member`'s chip over `range` (the open `@token`, or an insertion
    /// point) followed by one space; returns the caret after the space.
    @discardableResult
    static func insert(_ member: ChatMember, at range: Range<Int>, text: inout String, mentions: inout [ComposerMention]) -> Int {
        insert(.member(member), at: range, text: &text, mentions: &mentions)
    }

    @discardableResult
    static func insert(_ target: ComposerMentionTarget, at range: Range<Int>, text: inout String, mentions: inout [ComposerMention]) -> Int {
        let probe = ComposerMention(range: 0..<0, target: target)
        let chip = probe.plain
        let replaced = splice(&text, range, with: chip + " ", mentions: &mentions)
        let span = replaced.lowerBound..<(replaced.lowerBound + (chip as NSString).length)
        mentions.append(ComposerMention(range: span, target: target))
        mentions.sort { $0.range.lowerBound < $1.range.lowerBound }
        return span.upperBound + 1
    }

    /// Who `@handle` means among `members`, or nil when nobody or two kinds of
    /// entity carry it (#1538 — except my own handle, which means the agent).
    static func resolve(handle: String, members: [ChatMember], me: String?) -> ChatMember? {
        let slug = handle.lowercased()
        let hits = members.filter {
            ComposerMention.handle(of: $0).lowercased() == slug && (!$0.isAgent || $0.project != nil)
        }
        if hits.count == 1 { return hits[0] }
        if hits.count > 1, slug == me?.lowercased() { return hits.first { $0.isAgent } }
        return nil
    }

    /// `resolve` with the collectives: `@here` / `@agents` when `collectives`
    /// (a group or people chat), else a member.
    static func resolveTarget(handle: String, members: [ChatMember], me: String?, collectives: Bool) -> ComposerMentionTarget? {
        if collectives, let c = CollectiveMention(rawValue: handle.lowercased()) { return .collective(c) }
        return resolve(handle: handle, members: members, me: me).map { .member($0) }
    }

    /// Every plain `@handle` in `text` that `members` resolve (not already a
    /// chip) as a chip — a restored draft, or the text at send time.
    static func resolveAll(text: String, mentions: [ComposerMention], members: [ChatMember], me: String?, collectives: Bool = true) -> [ComposerMention] {
        var out = mentions
        let ns = text as NSString
        var i = 0
        while i < ns.length {
            let c = ns.character(at: i)
            guard c == 0x40, i == 0 || isSpace(ns.character(at: i - 1)) else { i += 1; continue }
            var end = i + 1
            while end < ns.length, isHandleChar(ns.character(at: end)) { end += 1 }
            while end > i + 1, [0x2E, 0x2D].contains(ns.character(at: end - 1)) { end -= 1 }
            if end > i + 1, !out.contains(where: { $0.range.lowerBound < end && $0.range.upperBound > i }),
               let t = resolveTarget(handle: ns.substring(with: NSRange(location: i + 1, length: end - i - 1)), members: members, me: me, collectives: collectives) {
                out.append(ComposerMention(range: i..<end, target: t))
            }
            i = max(end, i + 1)
        }
        out.sort { $0.range.lowerBound < $1.range.lowerBound }
        return out
    }

    /// Typed a separator at `caret`: the `@handle` right before it becomes a
    /// chip when `members` resolve it (behaviour 5).
    static func autoResolve(text: String, caret: Int, mentions: [ComposerMention], members: [ChatMember], me: String?, collectives: Bool = true) -> ComposerMention? {
        let ns = text as NSString
        guard caret >= 2, caret <= ns.length else { return nil }
        guard let sep = Unicode.Scalar(ns.character(at: caret - 1)), completesHandle(Character(sep)) else { return nil }
        var start = caret - 1
        while start > 0, isHandleChar(ns.character(at: start - 1)) { start -= 1 }
        guard start > 0, ns.character(at: start - 1) == 0x40 else { return nil }
        let at = start - 1
        guard at == 0 || isSpace(ns.character(at: at - 1)) else { return nil }
        var end = caret - 1
        while end > start, [0x2E, 0x2D].contains(ns.character(at: end - 1)) { end -= 1 }
        guard end > start, !mentions.contains(where: { $0.range.lowerBound < end && $0.range.upperBound > at }) else { return nil }
        guard let t = resolveTarget(handle: ns.substring(with: NSRange(location: start, length: end - start)), members: members, me: me, collectives: collectives) else { return nil }
        return ComposerMention(range: at..<end, target: t)
    }

    /// The collective rows for `query` (lower-case prefix), listed above the
    /// members: `@here` then `@agents`.
    static func collectiveCandidates(query: String) -> [CollectiveMention] {
        let q = query.lowercased()
        return CollectiveMention.allCases.filter { $0.handle.hasPrefix(q) }
    }

    /// The picker's rows for the typed `@query`: people and agents of the chat
    /// through the shared matcher on @handle and display name (`EntitySearch`,
    /// [feedback shared-search-matcher-ios-2026-10-26]: exact > prefix >
    /// word-start, name before handle, diacritics folded), then alphabetical by
    /// handle, agents first on a tie; never me; agents without a live project
    /// (soft-deleted) left out; at most six.
    static func candidates(query: String, members: [ChatMember], me: String?) -> [ChatMember] {
        var scored: [(ChatMember, UInt8)] = []
        for m in members {
            if m.isAgent, m.project == nil { continue }
            if !m.isAgent, let me, m.handle.lowercased() == me.lowercased() { continue }
            let handle = ComposerMention.handle(of: m)
            let name = m.isAgent ? (m.project?.name ?? m.handle) : (m.name ?? m.handle)
            guard let hit = EntitySearch.match(name: name, handle: handle, query: query) else { continue }
            scored.append((m, hit.order))
        }
        scored.sort { a, b in
            if a.1 != b.1 { return a.1 < b.1 }
            let ah = ComposerMention.handle(of: a.0).lowercased(), bh = ComposerMention.handle(of: b.0).lowercased()
            if ah != bh { return ah < bh }
            return a.0.isAgent && !b.0.isAgent
        }
        return Array(scored.prefix(6).map(\.0))
    }

    /// `agent` / `person` for a row whose handle another row shares (#1538).
    static func tag(for m: ChatMember, among rows: [ChatMember], me: String?) -> String? {
        let h = ComposerMention.handle(of: m).lowercased()
        let shared = rows.filter { ComposerMention.handle(of: $0).lowercased() == h }.count > 1 || h == me?.lowercased()
        return shared ? (m.isAgent ? "agent" : "person") : nil
    }

    /// Lower-case handle → member id for the chips, for the send's resolution.
    static func tokens(_ mentions: [ComposerMention]) -> [String: String] {
        var out: [String: String] = [:]
        for m in mentions { out[m.handle.lowercased()] = m.memberId }
        return out
    }

    private static func isSpace(_ c: unichar) -> Bool {
        guard let s = Unicode.Scalar(c) else { return false }
        return CharacterSet.whitespacesAndNewlines.contains(s)
    }

    private static func isHandleChar(_ c: unichar) -> Bool {
        guard let s = Unicode.Scalar(c) else { return false }
        return CharacterSet.alphanumerics.contains(s) || c == 0x5F || c == 0x2E || c == 0x2D
    }
}

// MARK: - The field

/// The composer's text field with mention chips: a `UITextView` whose chips
/// are text attachments. `text` is the plain draft (`@handle` where the chips
/// are), `mentions` the chip spans, `caret` the caret as a plain UTF-16 offset.
struct MentionTextView: UIViewRepresentable {
    @Binding var text: String
    @Binding var mentions: [ComposerMention]
    @Binding var caret: Int
    @Binding var isFocused: Bool
    let placeholder: String
    let theme: Theme
    /// The `Project` an agent chip draws its picture from — the app's single
    /// source for agent identity (`AppModel.agent(member:chatId:)`), so the chip
    /// matches the bubbles.
    var agentProject: (ChatMember) -> Project? = { $0.project }
    var maxLines: Int = 8

    /// Grok: the caret 17 pt in from the field's edge, one 22 pt line centred in 48.
    static let insets = UIEdgeInsets(top: Theme.composerTextPadY, left: Theme.composerTextInset, bottom: Theme.composerTextPadY, right: 0)

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> MentionUITextView {
        let view = MentionUITextView(frame: .zero, textContainer: nil)
        view.delegate = context.coordinator
        view.backgroundColor = .clear
        view.isScrollEnabled = false
        view.textContainerInset = Self.insets
        view.textContainer.lineFragmentPadding = 0
        view.keyboardDismissMode = .interactive
        view.adjustsFontForContentSizeCategory = false
        view.accessibilityIdentifier = "composer-field"
        view.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        view.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let coordinator = context.coordinator
        view.onMoveToWindow = { [weak view, weak coordinator] in
            guard let view, let coordinator, view.window != nil else { return }
            coordinator.syncFocus(view)
        }
        coordinator.apply(theme: theme, to: view)
        coordinator.rebuild(view, text: text, mentions: mentions, caret: caret)
        return view
    }

    func updateUIView(_ view: MentionUITextView, context: Context) {
        let c = context.coordinator
        c.parent = self
        c.apply(theme: theme, to: view)
        view.placeholderLabel.text = placeholder
        view.placeholderLabel.isHidden = !text.isEmpty
        view.placeholderLabel.font = c.font
        view.placeholderLabel.textColor = UIColor(theme.placeholder)
        if text != c.publishedText || mentions != c.publishedMentions {
            // Text changed from outside without a caret (a restored draft, the
            // rig typing into `draft`): the caret goes to the end.
            let target = (caret == c.publishedCaret && text != c.publishedText) ? (text as NSString).length : caret
            c.rebuild(view, text: text, mentions: mentions, caret: target)
            if target != caret {
                DispatchQueue.main.async { c.parent.caret = target }
            }
        } else if caret != c.publishedCaret {
            c.publishedCaret = caret
            let loc = min(c.attributedOffset(forPlain: caret, mentions: mentions), view.attributedText.length)
            if view.selectedRange.location != loc || view.selectedRange.length != 0 {
                c.rebuilding = true
                view.selectedRange = NSRange(location: loc, length: 0)
                c.rebuilding = false
            }
        }
        c.syncFocus(view)
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView: MentionUITextView, context: Context) -> CGSize? {
        // Greedy in width like `TextField` (the mic and the round button beside
        // it are fixed); the height hugs the text, one to `maxLines` lines.
        let width = proposal.width ?? max(uiView.bounds.width, 10)
        let measured = width.isFinite ? width : max(uiView.bounds.width, 10)
        let fit = uiView.sizeThatFits(CGSize(width: measured, height: .greatestFiniteMagnitude))
        let line = context.coordinator.font.lineHeight
        let minH = line + Self.insets.top + Self.insets.bottom
        let maxH = line * CGFloat(maxLines) + Self.insets.top + Self.insets.bottom
        let h = min(max(fit.height, minH), maxH)
        let scroll = fit.height > maxH + 0.5
        if uiView.isScrollEnabled != scroll { uiView.isScrollEnabled = scroll }
        return CGSize(width: width, height: h.rounded(.up))
    }

    @MainActor
    final class Coordinator: NSObject, UITextViewDelegate {
        var parent: MentionTextView
        var publishedText = ""
        var publishedMentions: [ComposerMention] = []
        var publishedCaret = 0
        /// Set while this side writes the view, so the delegate does not echo
        /// the change back into SwiftUI state mid-update.
        var rebuilding = false
        private(set) var font = UIFont.systemFont(ofSize: Theme.body)
        private var themeKey = ""
        private var chipCache: [String: UIImage] = [:]

        init(_ parent: MentionTextView) {
            self.parent = parent
        }

        /// First responder follows `isFocused` (both ways; see the delegate).
        func syncFocus(_ view: UITextView) {
            guard view.window != nil else { return }
            if parent.isFocused, !view.isFirstResponder {
                DispatchQueue.main.async { view.becomeFirstResponder() }
            } else if !parent.isFocused, view.isFirstResponder {
                DispatchQueue.main.async { view.resignFirstResponder() }
            }
        }

        var typing: [NSAttributedString.Key: Any] {
            [.font: font, .foregroundColor: UIColor(parent.theme.text)]
        }

        func apply(theme: Theme, to view: UITextView) {
            let key = "\(theme.isDark)|\(theme.textScale)"
            font = UIFont.systemFont(ofSize: theme.pt(Theme.body))
            // Only on a change: setting these restyles the whole text storage
            // and would disturb an in-progress IME composition.
            if view.font != font { view.font = font }
            let text = UIColor(theme.text)
            if view.textColor != text { view.textColor = text }
            let tint = UIColor(theme.accent)
            if view.tintColor != tint { view.tintColor = tint }
            view.typingAttributes = typing
            if key != themeKey {
                themeKey = key
                chipCache.removeAll()
                if !publishedText.isEmpty {
                    rebuild(view, text: publishedText, mentions: publishedMentions, caret: publishedCaret)
                }
            }
        }

        // MARK: plain ⇄ attributed

        /// A plain offset as an offset in the attributed string (each chip is one
        /// character there); an offset inside a chip maps to just after it.
        func attributedOffset(forPlain p: Int, mentions: [ComposerMention]) -> Int {
            var out = p
            for m in mentions {
                if m.range.upperBound <= p {
                    out -= m.range.count - 1
                } else if m.range.lowerBound < p {
                    out -= p - m.range.lowerBound - 1
                }
            }
            return max(0, out)
        }

        /// Rebuild the view's attributed text from the plain draft and its chips.
        func rebuild(_ view: UITextView, text: String, mentions: [ComposerMention], caret: Int) {
            let ns = text as NSString
            let out = NSMutableAttributedString()
            var pos = 0
            for m in mentions where m.range.lowerBound >= pos && m.range.upperBound <= ns.length {
                if m.range.lowerBound > pos {
                    out.append(NSAttributedString(string: ns.substring(with: NSRange(location: pos, length: m.range.lowerBound - pos)), attributes: typing))
                }
                out.append(NSAttributedString(attachment: attachment(for: m)))
                pos = m.range.upperBound
            }
            if pos < ns.length {
                out.append(NSAttributedString(string: ns.substring(from: pos), attributes: typing))
            }
            rebuilding = true
            view.attributedText = out
            view.typingAttributes = typing
            publishedText = text
            publishedMentions = mentions
            publishedCaret = caret
            let loc = min(attributedOffset(forPlain: caret, mentions: mentions), out.length)
            view.selectedRange = NSRange(location: loc, length: 0)
            rebuilding = false
            if let v = view as? MentionUITextView {
                v.placeholderLabel.isHidden = !text.isEmpty
            }
            view.invalidateIntrinsicContentSize()
        }

        /// Read the view back: the plain draft, the chip spans and the caret.
        func publish(from view: UITextView) {
            let attr = view.attributedText ?? NSAttributedString()
            var plain = ""
            var mentions: [ComposerMention] = []
            var caretPlain: Int?
            let sel = view.selectedRange.location
            var plainLen = 0
            attr.enumerateAttributes(in: NSRange(location: 0, length: attr.length)) { attrs, range, _ in
                if sel >= range.location, sel <= range.location + range.length, caretPlain == nil, attrs[.attachment] == nil {
                    caretPlain = plainLen + (sel - range.location)
                }
                if let a = attrs[.attachment] as? MentionAttachment {
                    if sel == range.location, caretPlain == nil { caretPlain = plainLen }
                    let chip = a.mention.plain
                    let len = (chip as NSString).length
                    mentions.append(ComposerMention(range: plainLen..<(plainLen + len), target: a.mention.target))
                    plain += chip
                    plainLen += len
                    if sel == range.location + range.length, caretPlain == nil { caretPlain = plainLen }
                } else {
                    let s = (attr.string as NSString).substring(with: range)
                    plain += s
                    plainLen += (s as NSString).length
                }
            }
            let caret = caretPlain ?? plainLen
            publishedText = plain
            publishedMentions = mentions
            publishedCaret = caret
            if parent.text != plain { parent.text = plain }
            if parent.mentions != mentions { parent.mentions = mentions }
            if parent.caret != caret { parent.caret = caret }
        }

        // MARK: chips

        private func attachment(for m: ComposerMention) -> MentionAttachment {
            let a = MentionAttachment(mention: m)
            let image = chipImage(for: m)
            a.image = image
            let padY = MentionTable.padY
            a.bounds = CGRect(x: 0, y: font.descender - padY, width: image.size.width, height: image.size.height)
            return a
        }

        /// The chip as one picture: picture · name on the accent pill, the
        /// bubble chip's geometry (`MentionTable`).
        private func chipImage(for m: ComposerMention) -> UIImage {
            let key = "\(m.memberId)|\(m.name)|\(themeKey)"
            if let hit = chipCache[key] { return hit }
            let theme = parent.theme
            let picture: AnyView
            switch m.target {
            case .member(let member) where member.isAgent:
                if let p = parent.agentProject(member) {
                    picture = AnyView(AgentAvatar(project: p, theme: theme, size: MentionTable.picture, still: true))
                } else {
                    picture = AnyView(InitialsAvatar(name: member.name ?? member.handle, handle: member.handle, id: member.id, size: MentionTable.picture))
                }
            case .member(let member):
                picture = AnyView(PersonAvatar(url: member.avatarUrl, handle: member.handle, size: MentionTable.picture, name: member.name, id: member.id, colorIndex: member.avatarColorIndex))
            case .collective(let c):
                // The same disc the bubble chip and the Mac draw: initials of `here` / `agents`.
                picture = AnyView(InitialsAvatar(name: c.handle, handle: c.handle, id: c.handle, size: MentionTable.picture))
            }
            let view = HStack(spacing: 4) {
                picture
                Text(m.name)
                    .font(.system(size: theme.pt(Theme.body), weight: .medium))
                    .foregroundStyle(theme.accent)
                    .lineLimit(1)
            }
            .padding(.leading, 4)
            .padding(.trailing, MentionTable.radius)
            .frame(height: font.lineHeight + MentionTable.padY * 2)
            .background(RoundedRectangle(cornerRadius: MentionTable.radius, style: .continuous).fill(theme.accent.opacity(0.14)))
            .environment(\.theme, theme)
            let renderer = ImageRenderer(content: view)
            renderer.scale = UIScreen.main.scale
            let image = renderer.uiImage ?? UIImage()
            chipCache[key] = image
            return image
        }

        // MARK: UITextViewDelegate

        func textViewDidChange(_ textView: UITextView) {
            guard !rebuilding else { return }
            textView.typingAttributes = typing
            if let v = textView as? MentionUITextView {
                v.placeholderLabel.isHidden = !(textView.text ?? "").isEmpty
            }
            textView.invalidateIntrinsicContentSize()
            publish(from: textView)
        }

        func textViewDidChangeSelection(_ textView: UITextView) {
            guard !rebuilding else { return }
            textView.typingAttributes = typing
            let sel = textView.selectedRange
            // The caret never sits "inside" a chip (one character), but a drag
            // selection may: that is fine, the chip goes whole with the selection.
            guard sel.length == 0 else { return }
            let caret = plainOffset(forAttributed: sel.location, in: textView)
            if caret != publishedCaret {
                publishedCaret = caret
                if parent.caret != caret { parent.caret = caret }
            }
        }

        func textViewDidBeginEditing(_ textView: UITextView) {
            if !parent.isFocused { parent.isFocused = true }
        }

        func textViewDidEndEditing(_ textView: UITextView) {
            if parent.isFocused { parent.isFocused = false }
        }

        func textView(_ textView: UITextView, shouldInteractWith textAttachment: NSTextAttachment, in characterRange: NSRange, interaction: UITextItemInteraction) -> Bool {
            false
        }

        private func plainOffset(forAttributed a: Int, in view: UITextView) -> Int {
            guard let attr = view.attributedText else { return a }
            var plain = 0
            var i = 0
            while i < min(a, attr.length) {
                if let att = attr.attribute(.attachment, at: i, effectiveRange: nil) as? MentionAttachment {
                    plain += (att.mention.plain as NSString).length
                } else {
                    plain += 1
                }
                i += 1
            }
            return plain
        }
    }
}

/// The field's `UITextView`: carries its own placeholder label and tells the
/// coordinator when it lands in a window (focus asked for before that).
final class MentionUITextView: UITextView {
    let placeholderLabel = UILabel()
    var onMoveToWindow: (() -> Void)?

    override init(frame: CGRect, textContainer: NSTextContainer?) {
        super.init(frame: frame, textContainer: textContainer)
        placeholderLabel.numberOfLines = 1
        placeholderLabel.isUserInteractionEnabled = false
        addSubview(placeholderLabel)
    }

    required init?(coder: NSCoder) {
        nil
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let inset = textContainerInset
        let line = placeholderLabel.font?.lineHeight ?? font?.lineHeight ?? 22
        placeholderLabel.frame = CGRect(x: inset.left, y: inset.top, width: max(0, bounds.width - inset.left - 8), height: line)
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        onMoveToWindow?()
    }
}

/// A chip in the field's text storage: one character that knows its member.
final class MentionAttachment: NSTextAttachment {
    let mention: ComposerMention

    init(mention: ComposerMention) {
        self.mention = mention
        super.init(data: nil, ofType: nil)
    }

    required init?(coder: NSCoder) {
        nil
    }
}
