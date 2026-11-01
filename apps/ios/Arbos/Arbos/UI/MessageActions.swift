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

    /// The first emoji the keyboard typed, if any. Every emoji the keyboard offers counts,
    /// including the text-default symbols it sends with U+FE0F (©️ ®️ ™️ ‼️ ⁉️ ℹ️ ⌨️, the
    /// arrows ↔️ … ↪️) and the keycaps (#️⃣ *️⃣ 0️⃣–9️⃣); a bare letter, digit or `#` does not
    /// ([feedback full-picker-reaction-ios-2026-10-29]: those picks were dropped).
    static func pickedEmoji(_ text: String) -> String? {
        guard let first = text.trimmingCharacters(in: .whitespaces).first,
              let lead = first.unicodeScalars.first, lead.properties.isEmoji else { return nil }
        let isEmoji = first.unicodeScalars.contains {
            $0.properties.isEmojiPresentation || $0.value == 0xFE0F || $0.value == 0x20E3
                || $0.properties.isEmoji && $0.value > 0x238C
        }
        return isEmoji ? String(first) : nil
    }
}

/// The add-emoji field's one answer. UIKit reports a single keyboard insert to the
/// field twice — `shouldChangeCharactersIn` first, then `.editingChanged` — and each
/// report toggled the reaction, so a pick from the emoji keyboard switched it on and
/// straight off again while the overlay closed on the first toggle
/// ([feedback iphone-emoji-picker-reaction-2026-10-28]). The first emoji wins; every
/// later report is dropped.
struct ReactionPick {
    private(set) var emoji: String?

    /// The emoji to react with, exactly once per field; nil for non-emoji text and for
    /// every report after the first pick.
    mutating func take(_ text: String) -> String? {
        guard emoji == nil, let picked = ReactionCapsule.pickedEmoji(text) else { return nil }
        emoji = picked
        return picked
    }
}

/// Where a row sits, for whoever has to know. Two rows report and neither in steady state:
/// the pressed one, for the actions overlay, and — only while a reveal is correcting its
/// scroll — the row that reveal is landing on (#2535).
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
        .overlay {
            if pickOpen {
                EmojiKeyboardField(onPick: onReact)
                    .frame(width: 1, height: 1)
                    .opacity(0.01)
                    .accessibilityIdentifier("reaction-pick-field")
            }
        }
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
            ForEach(ReactionCapsule.palette, id: \.self) { emoji in
                Button {
                    onReact(emoji)
                } label: {
                    Text(emoji)
                        .font(.system(size: ReactionCapsule.emojiSize))
                        .frame(width: ReactionCapsule.cell, height: ReactionCapsule.cell)
                        .background(Circle().fill(held.contains(where: { MessageReaction.sameBase($0, emoji) }) ? theme.accent.opacity(0.22) : Color.clear))
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

/// A one-character field that raises the **emoji** keyboard (the system picker
/// on iPhone): `textInputMode` is pinned to the emoji input mode. `onPick` is
/// called once, with the first emoji typed (`ReactionPick`).
struct EmojiKeyboardField: UIViewRepresentable {
    let onPick: (String) -> Void

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

    func makeCoordinator() -> Coordinator { Coordinator(onPick: onPick) }

    final class Coordinator: NSObject, UITextFieldDelegate {
        let onPick: (String) -> Void
        private var pick = ReactionPick()

        init(onPick: @escaping (String) -> Void) { self.onPick = onPick }

        /// Both delegate paths funnel here; `ReactionPick` lets only the first emoji through.
        func report(_ text: String) {
            if let emoji = pick.take(text) { onPick(emoji) }
        }

        @objc func changed(_ field: UITextField) {
            report(field.text ?? "")
        }

        // The emoji keyboard's insert arrives here first; `editingChanged` usually
        // follows with the same character, sometimes not (a non-text key).
        func textField(_ textField: UITextField, shouldChangeCharactersIn range: NSRange, replacementString string: String) -> Bool {
            report(string)
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
    /// A message bubble: long-press → actions overlay; double-tap → ❤️ reaction.
    /// Any other row (date / system / channel line) keeps a plain Copy context menu.
    @ViewBuilder
    func messageRowGestures(isMessage: Bool, onLongPress: @escaping () -> Void,
                            onDoubleTap: @escaping () -> Void,
                            onCopyMenu: @escaping (MessageMenuItem) -> Void) -> some View {
        if isMessage {
            // simultaneous so the chat ScrollView keeps vertical pans that start
            // on a bubble; exclusive onLongPressGesture eats the scroll
            // ([feedback app-01M5Q1N6C1HAKHC6P2XPEWMRSS]).
            // A hold on a chart plot reads a value instead (`ChartTouch`).
            simultaneousGesture(
                LongPressGesture(minimumDuration: 0.35)
                    .onEnded { _ in if !ChartTouch.holding { onLongPress() } },
                enabled: ChatGestures.longPress
            )
            // Double-tap toggles a heart under the bubble (Telegram / iMessage;
            // [feedback app-01M5VCGAPAXY6NN8TNSWAWJ52Z]).
            .simultaneousGesture(
                TapGesture(count: 2)
                    .onEnded { onDoubleTap() }
            )
        } else {
            contextMenu { MessageMenuContent(items: MessageMenu.copyOnly, action: onCopyMenu) }
        }
    }
}
