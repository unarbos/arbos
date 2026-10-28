// Hardware-keyboard ⌘F on the iPhone toggles the Search-in-chat bar
// ([feedback iphone-chat-search-cmd-f-2026-10-26] opened it, Jacob 2026-10-26;
// [feedback iphone-chat-search-cmd-f-toggle-2026-10-27] closes it again). The bar
// itself exists since #1829. The Mac twin is `ui/shell.rs` `chat_search::Open`.
//
// Sender chip + keyword ([feedback app-01M61GXS849K96HRER4YD0ZZ6R]): the bar
// filters by who wrote a message as well as by text, mirroring
// `crates/arbos-mac/src/chat_search.rs` (`Query::hits_row`, `offer_sender`).
// The bar UI lives here (not in ChatView) so ChatSession type-checks
// (swift-core-check on the hosted Mac).

import ArbosFFI
import Foundation
import SwiftUI
import UIKit

enum ChatSearchShortcut {
    /// ⌘F — the Find key on every Apple platform.
    static let key: KeyEquivalent = "f"
    static let modifiers: EventModifiers = .command

    /// What one ⌘F does to the bar. A toggle ([feedback
    /// iphone-chat-search-cmd-f-toggle-2026-10-27], Jacob 2026-10-27: "search
    /// should close with command F again"): closed → open and focus the field;
    /// open → close it exactly like ✕ / Esc. The Mac does the same.
    struct Effect: Equatable {
        /// The bar is shown afterwards.
        let open: Bool
        /// The field takes the cursor (only when opening).
        let focus: Bool
        /// Slide the bar in / out.
        let animate: Bool
    }

    static func apply(open: Bool) -> Effect {
        Effect(open: !open, focus: !open, animate: true)
    }
}

/// Search-in-chat query rules ([feedback app-01M61GXS849K96HRER4YD0ZZ6R]): a
/// keyword and an optional pinned sender. Pure so the tests reach it; the Mac
/// twin is `crates/arbos-mac/src/chat_search.rs`.
enum ChatSearchFilter {
    /// Someone who can write in the chat: the chip's name and handle, and the
    /// key a message's sender is compared by.
    struct Sender: Equatable, Hashable, Identifiable {
        /// Stable identity (`user:<id>`, `agent:<id>`) — never shown.
        var key: String
        var name: String
        /// Without the `@`; may be empty (an agent without a slug).
        var handle: String
        var id: String { key }
    }

    /// True while there is something to search for: a keyword and/or a pinned sender.
    static func isFiltering(keyword: String, pinned: Sender?) -> Bool {
        !EntitySearch.fold(keyword).isEmpty || pinned != nil
    }

    /// Does this message hit the query? With a pinned sender the message key
    /// must equal its key, AND the keyword is empty or the folded text contains
    /// the folded keyword. Empty keyword + a sender = all of that person's
    /// messages. Nothing typed and no sender = no hit (the view shows every row).
    static func hits(text: String, senderKey: String, keyword: String, pinned: Sender?) -> Bool {
        let foldedKeyword = EntitySearch.fold(keyword)
        if foldedKeyword.isEmpty && pinned == nil { return false }
        if let pinned, senderKey != pinned.key { return false }
        return foldedKeyword.isEmpty || EntitySearch.fold(text).contains(foldedKeyword)
    }

    /// The sender the typed text names, if any: best `EntitySearch.match` on
    /// name and handle (exact > prefix > word-start, name before handle). Nil
    /// for empty text or no match. Leading `@` is dropped by the matcher.
    static func offer(typed: String, senders: [Sender]) -> Sender? {
        if EntitySearch.normalize(typed).isEmpty { return nil }
        return senders
            .compactMap { s -> (UInt8, Sender)? in
                guard let m = EntitySearch.match(name: s.name, handle: s.handle, query: typed) else {
                    return nil
                }
                return (m.order, s)
            }
            .min(by: { $0.0 < $1.0 })
            .map(\.1)
    }

    /// Key a message by `Message.sender`; my own rows (`"user"` or `user:<my id>`)
    /// count as me (`me` is `user:<id>`).
    static func senderKey(_ sender: String, me: String?) -> String {
        if sender == "user" || sender.isEmpty { return me ?? "user" }
        if let me, sender == me { return me }
        return sender
    }

    /// Senders for the menu from a people chat's members: me included; agents
    /// without a project skipped (they cannot reply — same cut as member chips).
    static func senders(members: [ChatMember]) -> [Sender] {
        members.compactMap { m in
            if m.isAgent && m.project == nil { return nil }
            let handle = m.isAgent ? mentionSlug(m.project?.name ?? m.handle) : m.handle
            return Sender(key: m.member, name: m.displayName, handle: handle)
        }
    }
}

/// Search-in-chat bar: person Menu, pinned accent chip, keyword field, offer chip
/// ([feedback app-01M61GXS849K96HRER4YD0ZZ6R]). Lives outside `ChatSession` so
/// that view's `sessionRoot` type-checks.
struct ChatSearchBar<Avatar: View>: View {
    @Environment(\.theme) private var theme
    @Binding var text: String
    @Binding var pinned: ChatSearchFilter.Sender?
    @Binding var focused: Bool
    let senders: [ChatSearchFilter.Sender]
    var onClose: () -> Void
    var onPin: (ChatSearchFilter.Sender) -> Void
    var onUnpin: () -> Void
    @ViewBuilder var avatar: (ChatSearchFilter.Sender) -> Avatar

    private var offered: ChatSearchFilter.Sender? {
        guard pinned == nil else { return nil }
        return ChatSearchFilter.offer(typed: text, senders: senders)
    }

    var body: some View {
        VStack(spacing: 6) {
            fieldRow
            if let offered {
                offerChip(offered)
            }
        }
        .padding(.horizontal, Controls.inset)
    }

    private var fieldRow: some View {
        HStack(spacing: 8) {
            senderMenu
            if let pinned {
                pinnedChip(pinned)
            }
            ChatSearchTextField(
                text: $text,
                isFocused: $focused,
                placeholder: "Search in chat",
                placeholderColor: UIColor(theme.placeholder),
                textColor: UIColor(theme.text),
                font: UIFont.systemFont(ofSize: Theme.body * theme.textScale),
                onDeleteBackwardEmpty: {
                    if text.isEmpty { onUnpin() }
                })
            .frame(maxWidth: .infinity, minHeight: 22)
            Button(action: onClose) {
                Image(systemName: "xmark.circle.fill").foregroundStyle(theme.muted)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Close search")
        }
        .padding(.horizontal, Controls.textInset).frame(height: Controls.height)
        .background {
            HeaderGlassBackground(
                shape: Capsule(),
                material: ChatHeaderGlass.searchBarMaterial(surface: theme.surface),
                tinted: true)
        }
    }

    private var senderMenu: some View {
        Menu {
            ForEach(senders) { sender in
                Button {
                    onPin(sender)
                } label: {
                    Label {
                        Text(Self.title(sender))
                    } icon: {
                        avatar(sender)
                    }
                }
            }
        } label: {
            Image(systemName: "person.crop.circle")
                .font(.system(size: Controls.glyph, weight: .medium))
                .foregroundStyle(pinned != nil ? theme.accent : theme.muted)
        }
        .accessibilityLabel("Filter by sender")
    }

    private static func title(_ sender: ChatSearchFilter.Sender) -> String {
        if sender.handle.isEmpty { return sender.name }
        return "\(sender.name)  @\(sender.handle)"
    }

    private func pinnedChip(_ pinned: ChatSearchFilter.Sender) -> some View {
        HStack(spacing: 4) {
            Text("From \(pinned.name)")
                .font(theme.font(12.5, weight: .medium))
                .foregroundStyle(theme.accent)
                .lineLimit(1)
            Button(action: onUnpin) {
                Image(systemName: "xmark")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundStyle(theme.accent)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Clear sender filter")
        }
        .padding(.leading, 8).padding(.trailing, 6).padding(.vertical, 4)
        .background(Capsule().fill(theme.accent.opacity(0.18)))
    }

    private func offerChip(_ offered: ChatSearchFilter.Sender) -> some View {
        Button {
            onPin(offered)
        } label: {
            HStack(spacing: 6) {
                avatar(offered)
                Text("From \(offered.name)")
                    .font(theme.font(12.5, weight: .medium))
                    .foregroundStyle(theme.text)
                    .lineLimit(1)
                if !offered.handle.isEmpty {
                    Text("@\(offered.handle)")
                        .font(theme.font(12.5))
                        .foregroundStyle(theme.muted)
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 12).padding(.vertical, 6)
            .background(
                Capsule()
                    .fill(theme.text.opacity(0.08))
                    .overlay(Capsule().strokeBorder(theme.separator, lineWidth: 1))
            )
        }
        .buttonStyle(.plain)
        .padding(.horizontal, Controls.textInset)
        .accessibilityLabel("Filter from \(offered.name)")
    }
}

/// Search-in-chat field: ⌫ on an empty field unpins the sender chip
/// ([feedback app-01M61GXS849K96HRER4YD0ZZ6R]; Mac `chat_search_unpin_sender`).
struct ChatSearchTextField: UIViewRepresentable {
    @Binding var text: String
    @Binding var isFocused: Bool
    var placeholder: String
    var placeholderColor: UIColor
    var textColor: UIColor
    var font: UIFont
    var onDeleteBackwardEmpty: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> ChatSearchUITextField {
        let field = ChatSearchUITextField()
        field.delegate = context.coordinator
        field.onDeleteBackwardEmpty = { [weak field] in
            guard let field, (field.text ?? "").isEmpty else { return }
            context.coordinator.parent.onDeleteBackwardEmpty()
        }
        field.borderStyle = .none
        field.backgroundColor = .clear
        field.autocorrectionType = .no
        field.autocapitalizationType = .none
        field.returnKeyType = .search
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        field.addTarget(context.coordinator, action: #selector(Coordinator.changed(_:)), for: .editingChanged)
        return field
    }

    func updateUIView(_ uiView: ChatSearchUITextField, context: Context) {
        context.coordinator.parent = self
        if uiView.text != text { uiView.text = text }
        uiView.font = font
        uiView.textColor = textColor
        uiView.attributedPlaceholder = NSAttributedString(
            string: placeholder,
            attributes: [.foregroundColor: placeholderColor, .font: font])
        uiView.onDeleteBackwardEmpty = { [weak uiView] in
            guard let uiView, (uiView.text ?? "").isEmpty else { return }
            context.coordinator.parent.onDeleteBackwardEmpty()
        }
        if isFocused {
            if !uiView.isFirstResponder { uiView.becomeFirstResponder() }
        } else if uiView.isFirstResponder {
            uiView.resignFirstResponder()
        }
    }

    final class Coordinator: NSObject, UITextFieldDelegate {
        var parent: ChatSearchTextField
        init(_ parent: ChatSearchTextField) { self.parent = parent }

        @objc func changed(_ field: UITextField) {
            let next = field.text ?? ""
            if parent.text != next { parent.text = next }
        }

        func textFieldDidBeginEditing(_ textField: UITextField) {
            if !parent.isFocused { parent.isFocused = true }
        }

        func textFieldDidEndEditing(_ textField: UITextField) {
            if parent.isFocused { parent.isFocused = false }
        }

        func textFieldShouldReturn(_ textField: UITextField) -> Bool {
            textField.resignFirstResponder()
            return true
        }
    }
}

final class ChatSearchUITextField: UITextField {
    var onDeleteBackwardEmpty: (() -> Void)?

    override func deleteBackward() {
        if (text ?? "").isEmpty {
            onDeleteBackwardEmpty?()
            return
        }
        super.deleteBackward()
    }
}
