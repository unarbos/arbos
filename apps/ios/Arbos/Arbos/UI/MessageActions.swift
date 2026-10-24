import SwiftUI
import UIKit

/// Long-press on a bubble (Grok Bot / iMessage shape, Jacob 2026-10-23): the chat
/// blurs, the bubble stays where it is, the **reaction capsule** sits above it
/// (😂 👍 👎 ❤️ 🎉 + add-emoji, which raises the system emoji keyboard) and the
/// message menu (Copy · Reply · Delete, `MessageMenu`) below it. The Mac's
/// `ui/reactions.rs` is the same capsule behind the hover rail's smiley.
enum ReactionCapsule {
    /// Grok's five, in its order; anything else comes from the emoji keyboard.
    static var palette: [String] { MessageReaction.palette }
    static let cell: CGFloat = 44
    static let emojiSize: CGFloat = 26
    static let padding: CGFloat = 4
    static var width: CGFloat { 2 * padding + CGFloat(palette.count + 1) * cell }
    static var height: CGFloat { 2 * padding + cell }

    /// The first emoji the keyboard typed, if any.
    static func pickedEmoji(_ text: String) -> String? {
        guard let first = text.trimmingCharacters(in: .whitespaces).first else { return nil }
        let isEmoji = first.unicodeScalars.contains { $0.properties.isEmojiPresentation || $0.properties.isEmoji && $0.value > 0x238C }
        return isEmoji ? String(first) : nil
    }
}

/// Where each row sits, for the overlay (only the pressed row reports).
struct RowFrameKey: PreferenceKey {
    static let defaultValue: [String: Anchor<CGRect>] = [:]
    static func reduce(value: inout [String: Anchor<CGRect>], nextValue: () -> [String: Anchor<CGRect>]) {
        value.merge(nextValue(), uniquingKeysWith: { $1 })
    }
}

struct MessageActionsOverlay<Bubble: View>: View {
    @Environment(\.theme) private var theme
    let row: Row
    /// The bubble's frame in the overlay's space.
    let frame: CGRect
    let width: CGFloat
    /// The overlay's own height (the chat view), for clamping.
    let height: CGFloat
    let safeTop: CGFloat
    let safeBottom: CGFloat
    /// Emoji I already hold on this message (tinted in the capsule).
    let held: Set<String>
    let items: [MessageMenuItem]
    let onReact: (String) -> Void
    let onMenu: (MessageMenuItem) -> Void
    let onDismiss: () -> Void
    @ViewBuilder let bubble: () -> Bubble

    @State private var pickOpen = false
    @State private var picked = ""

    private var mine: Bool { row.role == .user }
    private var gap: CGFloat { 8 }
    private var menuWidth: CGFloat { 250 }
    private var menuHeight: CGFloat { CGFloat(items.count) * 44 + (items.contains(.delete) ? 8 : 0) }

    var body: some View {
        let groupHeight = ReactionCapsule.height + gap + frame.height + gap + menuHeight
        let wanted = frame.minY - ReactionCapsule.height - gap
        let lowest = max(safeTop + 8, height - safeBottom - 8 - groupHeight)
        let top = min(max(wanted, safeTop + 8), lowest)
        ZStack(alignment: .topLeading) {
            Rectangle()
                .fill(.ultraThinMaterial)
                .overlay(Color.black.opacity(theme.isDark ? 0.35 : 0.12))
                .ignoresSafeArea()
                .onTapGesture { onDismiss() }
            VStack(alignment: mine ? .trailing : .leading, spacing: gap) {
                capsule
                bubble()
                    .frame(width: width)
                menu
            }
            .frame(width: width, alignment: mine ? .trailing : .leading)
            .offset(y: top)
            if pickOpen {
                EmojiKeyboardField(text: $picked) { text in
                    if let emoji = ReactionCapsule.pickedEmoji(text) { onReact(emoji) }
                }
                .frame(width: 1, height: 1)
                .opacity(0.01)
            }
        }
        .transition(.opacity)
    }

    private var capsule: some View {
        HStack(spacing: 0) {
            ForEach(ReactionCapsule.palette, id: \.self) { emoji in
                Button {
                    onReact(emoji)
                } label: {
                    Text(emoji)
                        .font(.system(size: ReactionCapsule.emojiSize))
                        .frame(width: ReactionCapsule.cell, height: ReactionCapsule.cell)
                        .background(Circle().fill(held.contains(emoji) ? theme.accent.opacity(0.22) : Color.clear))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("React \(emoji)")
            }
            Button {
                pickOpen = true
            } label: {
                ZStack(alignment: .topTrailing) {
                    Image(systemName: "face.smiling")
                        .font(.system(size: 24, weight: .regular))
                        .foregroundStyle(theme.muted)
                    Image(systemName: "plus.circle.fill")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundStyle(theme.text, theme.card)
                        .offset(x: 4, y: -3)
                }
                .frame(width: ReactionCapsule.cell, height: ReactionCapsule.cell)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("More emoji")
        }
        .padding(ReactionCapsule.padding)
        .background(Capsule(style: .continuous).fill(theme.card))
        .overlay(Capsule(style: .continuous).strokeBorder(theme.separator, lineWidth: 1))
        .shadow(color: .black.opacity(0.35), radius: 14, y: 6)
        .padding(.horizontal, 16)
    }

    private var menu: some View {
        VStack(spacing: 0) {
            ForEach(Array(items.enumerated()), id: \.element) { i, item in
                if item.isDestructive, i > 0 {
                    Rectangle().fill(theme.bg.opacity(0.6)).frame(height: 8)
                }
                Button {
                    onMenu(item)
                } label: {
                    HStack(spacing: 12) {
                        Image(systemName: item.systemImage)
                            .font(.system(size: 17))
                            .frame(width: 22)
                        Text(item.label).font(theme.font(Theme.body))
                        Spacer(minLength: 0)
                    }
                    .foregroundStyle(item.isDestructive ? theme.danger : theme.text)
                    .padding(.horizontal, 16)
                    .frame(height: 44)
                }
                .buttonStyle(.plain)
                if i + 1 < items.count, !items[i + 1].isDestructive {
                    Rectangle().fill(theme.separator).frame(height: 0.5).padding(.leading, 50)
                }
            }
        }
        .frame(width: menuWidth)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(theme.card))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(theme.separator, lineWidth: 1))
        .shadow(color: .black.opacity(0.35), radius: 14, y: 6)
        .padding(.horizontal, 16)
    }
}

/// A one-character field that raises the **emoji** keyboard (the system picker
/// on iPhone): `textInputMode` is pinned to the emoji input mode.
struct EmojiKeyboardField: UIViewRepresentable {
    @Binding var text: String
    let onChange: (String) -> Void

    func makeUIView(context: Context) -> EmojiTextField {
        let field = EmojiTextField()
        field.delegate = context.coordinator
        field.addTarget(context.coordinator, action: #selector(Coordinator.changed(_:)), for: .editingChanged)
        field.autocorrectionType = .no
        field.spellCheckingType = .no
        field.tintColor = .clear
        DispatchQueue.main.async { field.becomeFirstResponder() }
        return field
    }

    func updateUIView(_ uiView: EmojiTextField, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    final class Coordinator: NSObject, UITextFieldDelegate {
        var parent: EmojiKeyboardField
        init(_ parent: EmojiKeyboardField) { self.parent = parent }
        @objc func changed(_ field: UITextField) {
            parent.text = field.text ?? ""
            parent.onChange(parent.text)
        }
        // The emoji keyboard's insert arrives here first; `editingChanged` does
        // not always follow for a non-text key.
        func textField(_ textField: UITextField, shouldChangeCharactersIn range: NSRange, replacementString string: String) -> Bool {
            if !string.isEmpty {
                parent.text = string
                parent.onChange(string)
            }
            return true
        }
    }
}

final class EmojiTextField: UITextField {
    override var textInputMode: UITextInputMode? {
        UITextInputMode.activeInputModes.first { $0.primaryLanguage == "emoji" } ?? super.textInputMode
    }
    override var textInputContextIdentifier: String? { "arbos.reaction.emoji" }
}

extension View {
    /// A message bubble long-presses into the actions overlay; any other row
    /// (date / system / channel line) keeps a plain Copy context menu.
    @ViewBuilder
    func messageRowGestures(isMessage: Bool, onLongPress: @escaping () -> Void,
                            onCopyMenu: @escaping (MessageMenuItem) -> Void) -> some View {
        if isMessage {
            onLongPressGesture(minimumDuration: 0.35, perform: onLongPress)
        } else {
            contextMenu { MessageMenuContent(items: MessageMenu.copyOnly, action: onCopyMenu) }
        }
    }
}
