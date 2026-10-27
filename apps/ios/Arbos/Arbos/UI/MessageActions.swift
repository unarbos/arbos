import SwiftUI
import UIKit

/// Long-press on a bubble (Grok Bot / iMessage shape, Jacob 2026-10-23): the chat
/// blurs, the bubble stays where it is, the **reaction capsule** sits above it
/// (❤️ 👍🏽 💪🏽 👎🏻 👌🏾 🤝 🔥 💯 + add-emoji, which raises the system emoji
/// keyboard) and the message menu (Copy · Reply · Delete, `MessageMenu`) below
/// it. The Mac's `ui/reactions.rs` is the same capsule behind the hover rail's
/// smiley. The composer keyboard goes down as the overlay opens and the group is
/// fitted to the visible area by `MessageActionsLayout` (menu flips above the
/// bubble when there is no room below).
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
    /// The bubble's frame when the press landed. The composer's keyboard goes down as
    /// the overlay opens and the transcript reflows under the blur; the held bubble
    /// stays where the finger is (iMessage) instead of following that reflow.
    @State private var pressedFrame: CGRect?
    @ObservedObject private var keyboard = KeyboardFrameObserver.shared

    private var mine: Bool { row.role == .user }
    private var gap: CGFloat { 8 }
    private var menuWidth: CGFloat { 250 }
    /// The menu's height is its rows' (the same geometry `menu` draws and hit-tests).
    private var menuHeight: CGFloat { MessageActionsMenuRows.compute(items: items, width: menuWidth).height }

    var body: some View {
        GeometryReader { geo in
            let bubbleFrame = pressedFrame ?? frame
            let layout = MessageActionsLayout.compute(.init(
                bubble: bubbleFrame,
                capsuleHeight: ReactionCapsule.height,
                menuHeight: menuHeight,
                gap: gap,
                height: height,
                safeTop: safeTop,
                safeBottom: safeBottom,
                keyboardInset: keyboard.inset(overlayFrame: geo.frame(in: .global))
            ))
            ZStack(alignment: .topLeading) {
                Rectangle()
                    .fill(.ultraThinMaterial)
                    .overlay(Color.black.opacity(theme.isDark ? 0.35 : 0.12))
                    .ignoresSafeArea()
                    .onTapGesture { onDismiss() }
                capsule
                    .frame(width: width, alignment: mine ? .trailing : .leading)
                    .offset(y: layout.capsuleY)
                    .accessibilityIdentifier("message-actions-capsule")
                bubble()
                    .frame(width: width)
                    // Only a bubble taller than the visible area is cut (layout rule 5).
                    .frame(height: layout.bubbleHeight < bubbleFrame.height ? layout.bubbleHeight : nil, alignment: .top)
                    .clipped()
                    .offset(y: layout.bubbleY)
                menu
                    .frame(width: width, alignment: mine ? .trailing : .leading)
                    .offset(y: layout.menuY)
                    .accessibilityIdentifier("message-actions-menu")
            }
            .animation(.easeOut(duration: 0.2), value: layout)
            .onAppear { if pressedFrame == nil { pressedFrame = frame } }
        }
        .ignoresSafeArea()
        // Pick field lives in the capsule (not a 1×1 hidden overlay): when
        // pickOpen the palette yields to EmojiKeyboardField so any emoji works.
        .transition(.opacity)
    }

    // Every cell and every menu row is tappable over its WHOLE rect (feedback
    // iphone-bubble-menu-hit-area-2026-10-26): a `.plain` Button only hit-tests the drawn
    // glyphs — not `.frame` padding, not a `Spacer`, not a `Color.clear` background — so
    // taps between the icon and the right edge of a row, or beside an emoji, fell
    // through. `.contentShape(Rectangle())` over the full frame fixes that; the pressed
    // highlight comes from `MenuRowButtonStyle` / `CapsuleCellButtonStyle`.

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
                        .contentShape(Rectangle())
                }
                .buttonStyle(CapsuleCellButtonStyle(pressed: theme.text.opacity(0.1)))
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
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(CapsuleCellButtonStyle(pressed: theme.text.opacity(0.1)))
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
                    .contentShape(Rectangle())
                }
                .buttonStyle(CapsuleCellButtonStyle(pressed: theme.text.opacity(0.1)))
                .accessibilityLabel("More emoji")
            }
        }
        .padding(ReactionCapsule.padding)
        .background(Capsule(style: .continuous).fill(theme.card))
        .overlay(Capsule(style: .continuous).strokeBorder(theme.separator, lineWidth: 1))
        .overlay { hitRectsOverlay(ReactionCapsule.cellFrames) }
        .shadow(color: .black.opacity(0.35), radius: 14, y: 6)
        .padding(.horizontal, 16)
    }

    private var menu: some View {
        let rows = MessageActionsMenuRows.compute(items: items, width: menuWidth)
        return VStack(spacing: 0) {
            ForEach(rows.rows, id: \.item) { row in
                Button {
                    onMenu(row.item)
                } label: {
                    VStack(spacing: 0) {
                        if row.bandAbove > 0 {
                            Rectangle().fill(theme.bg.opacity(0.6)).frame(height: row.bandAbove)
                        }
                        HStack(spacing: 12) {
                            Image(systemName: row.item.systemImage)
                                .font(.system(size: 17))
                                .frame(width: 22)
                            Text(row.item.label).font(theme.font(Theme.body))
                            Spacer(minLength: 0)
                        }
                        .foregroundStyle(row.item.isDestructive ? theme.danger : theme.text)
                        .padding(.horizontal, 16)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .frame(height: MessageActionsMenuRows.rowHeight)
                    }
                    .frame(width: row.frame.width, height: row.frame.height)
                    .overlay(alignment: .bottom) {
                        if row.dividerBelow {
                            Rectangle().fill(theme.separator)
                                .frame(height: MessageActionsMenuRows.dividerHeight)
                                .padding(.leading, MessageActionsMenuRows.dividerInset)
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(MenuRowButtonStyle(pressed: theme.text.opacity(0.08), bandAbove: row.bandAbove))
                .accessibilityIdentifier("message-menu-\(row.item.label.lowercased().replacingOccurrences(of: " ", with: "-"))")
            }
        }
        .frame(width: menuWidth, height: rows.height)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(theme.card))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(theme.separator, lineWidth: 1))
        .overlay { hitRectsOverlay(rows.rows.map(\.hitRect)) }
        .shadow(color: .black.opacity(0.35), radius: 14, y: 6)
        .padding(.horizontal, 16)
    }

    /// Rig only (`ARBOS_HIT_RECTS=1`): strokes each hit rect so a shot shows what a tap reaches.
    @ViewBuilder
    private func hitRectsOverlay(_ rects: [CGRect]) -> some View {
        if DebugHooks.hitRects {
            ZStack(alignment: .topLeading) {
                ForEach(Array(rects.enumerated()), id: \.offset) { _, r in
                    Rectangle()
                        .strokeBorder(Color.green, lineWidth: 1.5)
                        .background(Color.green.opacity(0.12))
                        .frame(width: r.width, height: r.height)
                        .offset(x: r.minX, y: r.minY)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .allowsHitTesting(false)
        }
    }
}

/// A menu row's pressed state: the label row tints, the destructive band above it does not.
struct MenuRowButtonStyle: ButtonStyle {
    let pressed: Color
    let bandAbove: CGFloat

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(alignment: .bottom) {
                if configuration.isPressed {
                    Rectangle().fill(pressed).frame(height: MessageActionsMenuRows.rowHeight)
                }
            }
    }
}

/// A capsule cell's pressed state: a soft disc under the emoji.
struct CapsuleCellButtonStyle: ButtonStyle {
    let pressed: Color

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(Circle().fill(configuration.isPressed ? pressed : Color.clear))
            .scaleEffect(configuration.isPressed ? 1.12 : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

/// Where the keyboard is on screen, from the moment the app starts, so the
/// actions overlay keeps the capsule and the menu above it — the system keyboard
/// while it animates down as the menu opens, the emoji keyboard the add-emoji
/// control raises (feedback iphone-bubble-menu-keyboard-2026-10-25).
@MainActor
final class KeyboardFrameObserver: ObservableObject {
    static let shared = KeyboardFrameObserver()

    /// The keyboard's end frame in screen coordinates; `nil` when it is down.
    @Published private(set) var frame: CGRect?

    private var tokens: [NSObjectProtocol] = []

    init() {
        let center = NotificationCenter.default
        tokens.append(center.addObserver(forName: UIResponder.keyboardWillChangeFrameNotification, object: nil, queue: .main) { [weak self] note in
            // Documented as the *end* frame: a dismissal reports a frame below the screen.
            let end = (note.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? NSValue)?.cgRectValue
            MainActor.assumeIsolated { self?.update(end) }
        })
        tokens.append(center.addObserver(forName: UIResponder.keyboardWillHideNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.update(nil) }
        })
    }

    private func update(_ end: CGRect?) {
        guard let end, let screen = UIApplication.shared.connectedScenes
            .compactMap({ ($0 as? UIWindowScene)?.screen }).first,
              end.minY < screen.bounds.maxY, end.height > 0
        else { frame = nil; return }
        frame = end
    }

    /// How far the keyboard reaches up into an overlay whose frame (in `.global`
    /// space) is `overlayFrame`; 0 when the keyboard is down or below it.
    func inset(overlayFrame: CGRect) -> CGFloat {
        guard let frame else { return 0 }
        return max(0, overlayFrame.maxY - frame.minY)
    }

    /// The window's safe area (status bar / notch, home indicator). A `GeometryReader`
    /// under `.ignoresSafeArea()` reports zero insets, so the full-screen overlay asks
    /// the window instead.
    static var windowSafeArea: UIEdgeInsets {
        let window = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap(\.windows)
            .first { $0.isKeyWindow }
        return window?.safeAreaInsets ?? .zero
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
                    .onEnded { _ in onLongPress() },
                enabled: ChatGestures.longPress
            )
        } else {
            contextMenu { MessageMenuContent(items: MessageMenu.copyOnly, action: onCopyMenu) }
        }
    }
}
