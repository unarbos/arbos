import SwiftUI
import UIKit

/// Long-press on a bubble (Grok Bot / iMessage shape, Jacob 2026-10-23): the chat
/// blurs, the bubble stays where it is, the **reaction capsule** sits above it
/// (❤️ 👍🏽 💪🏽 👎🏻 👌🏾 🤝 🔥 💯 + add-emoji, which raises the system emoji
/// keyboard) and the message menu (Copy · Reply · Delete, `MessageMenu`) below
/// it. The Mac's `ui/reactions.rs` is the same capsule behind the hover rail's
/// smiley.
enum ReactionCapsule {
    /// Quick-react set; anything else comes from the emoji keyboard.
    static var palette: [String] { MessageReaction.palette }
    /// Sized so eight palette cells plus the add-emoji control fit a phone width.
    static let cell: CGFloat = 36
    static let emojiSize: CGFloat = 22
    static let padding: CGFloat = 4
    static var width: CGFloat { 2 * padding + CGFloat(palette.count + 1) * cell }
    static var height: CGFloat { 2 * padding + cell }

    /// Width of the "pick any emoji" field that replaces the palette while the
    /// system emoji keyboard is up (Mac `PICK_FIELD_WIDTH`).
    static let pickFieldWidth: CGFloat = 160

    /// The first emoji grapheme the keyboard typed, if any. Uses the full
    /// extended grapheme cluster so skin-tone and ZWJ sequences stay intact.
    static func pickedEmoji(_ text: String) -> String? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let first = trimmed.first else { return nil }
        let grapheme = String(first)
        return isEmojiGrapheme(grapheme) ? grapheme : nil
    }

    /// True when `s` is a single emoji grapheme (presentation emoji, emoji
    /// scalar, or a sequence held together by ZWJ / VS16 — same idea as Mac
    /// `picked_emoji`).
    static func isEmojiGrapheme(_ s: String) -> Bool {
        for scalar in s.unicodeScalars {
            let v = scalar.value
            if scalar.properties.isEmojiPresentation { return true }
            if scalar.properties.isEmoji && v > 0x238C { return true }
            if (0x1F000...0x1FAFF).contains(v)
                || (0x2600...0x27BF).contains(v)
                || (0x2B00...0x2BFF).contains(v)
                || (0x1F900...0x1F9FF).contains(v)
                || (0x2190...0x21FF).contains(v)
                || v == 0x200D
                || v == 0xFE0F
            {
                return true
            }
        }
        return false
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
        }
        .transition(.opacity)
    }

    private var capsule: some View {
        HStack(spacing: 0) {
            if pickOpen {
                // Mac parity: the palette yields to a real pick field so the
                // system emoji keyboard has a focused target (a 1×1 hidden
                // field often never raised the keyboard — testers only saw
                // the eight preview emojis).
                EmojiKeyboardField(
                    text: $picked,
                    placeholder: "Pick an emoji…",
                    textColor: UIColor(theme.text),
                    placeholderColor: UIColor(theme.placeholder)
                ) { text in
                    if let emoji = ReactionCapsule.pickedEmoji(text) { onReact(emoji) }
                }
                .frame(width: ReactionCapsule.pickFieldWidth, height: ReactionCapsule.cell)
                .padding(.leading, 6)
                Button {
                    pickOpen = false
                    picked = ""
                } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(theme.muted)
                        .frame(width: ReactionCapsule.cell, height: ReactionCapsule.cell)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Close emoji picker")
            } else {
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

/// A pick field that raises the **emoji** keyboard (the system picker on
/// iPhone): `textInputMode` is pinned to the emoji input mode, and the field
/// sits in the capsule so focus is real (not a hidden 1×1 stub).
struct EmojiKeyboardField: UIViewRepresentable {
    @Binding var text: String
    var placeholder: String = "Pick an emoji…"
    var textColor: UIColor = .label
    var placeholderColor: UIColor = .secondaryLabel
    let onChange: (String) -> Void

    func makeUIView(context: Context) -> EmojiTextField {
        let field = EmojiTextField()
        field.delegate = context.coordinator
        field.addTarget(context.coordinator, action: #selector(Coordinator.changed(_:)), for: .editingChanged)
        field.autocorrectionType = .no
        field.spellCheckingType = .no
        field.returnKeyType = .done
        field.tintColor = textColor
        field.textColor = textColor
        field.font = .systemFont(ofSize: 17)
        field.backgroundColor = .clear
        field.borderStyle = .none
        field.attributedPlaceholder = NSAttributedString(
            string: placeholder,
            attributes: [.foregroundColor: placeholderColor]
        )
        // Focus after the capsule has laid the field out; a same-run call often
        // fails when the representable was just inserted.
        DispatchQueue.main.async { _ = field.becomeFirstResponder() }
        return field
    }

    func updateUIView(_ uiView: EmojiTextField, context: Context) {
        context.coordinator.parent = self
        uiView.textColor = textColor
        uiView.tintColor = textColor
        uiView.attributedPlaceholder = NSAttributedString(
            string: placeholder,
            attributes: [.foregroundColor: placeholderColor]
        )
    }

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    final class Coordinator: NSObject, UITextFieldDelegate {
        var parent: EmojiKeyboardField
        init(_ parent: EmojiKeyboardField) { self.parent = parent }
        @objc func changed(_ field: UITextField) {
            parent.text = field.text ?? ""
            parent.onChange(parent.text)
        }
        // The emoji keyboard's insert arrives here first; `editingChanged` does
        // not always follow for a non-text key. Consume the insert so a failed
        // (non-emoji) keystroke does not linger in the field.
        func textField(_ textField: UITextField, shouldChangeCharactersIn range: NSRange, replacementString string: String) -> Bool {
            guard !string.isEmpty else { return true }
            parent.text = string
            parent.onChange(string)
            return false
        }
    }
}

final class EmojiTextField: UITextField {
    override var textInputMode: UITextInputMode? {
        UITextInputMode.activeInputModes.first { $0.primaryLanguage == "emoji" } ?? super.textInputMode
    }
    override var textInputContextIdentifier: String? { "arbos.reaction.emoji" }

    @discardableResult
    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        // Re-ask for the emoji input mode after UIKit attaches the keyboard.
        if ok { reloadInputViews() }
        return ok
    }
}

extension View {
    /// A message bubble long-presses into the actions overlay; any other row
    /// (date / system / channel line) keeps a plain Copy context menu.
    @ViewBuilder
    func messageRowGestures(isMessage: Bool, onLongPress: @escaping () -> Void,
                            onCopyMenu: @escaping (MessageMenuItem) -> Void) -> some View {
        if isMessage {
            // simultaneous so the chat ScrollView keeps vertical pans that start
            // on a bubble; exclusive onLongPressGesture eats the scroll
            // ([feedback app-01M5Q1N6C1HAKHC6P2XPEWMRSS]).
            simultaneousGesture(
                LongPressGesture(minimumDuration: 0.35)
                    .onEnded { _ in onLongPress() }
            )
        } else {
            contextMenu { MessageMenuContent(items: MessageMenu.copyOnly, action: onCopyMenu) }
        }
    }
}
