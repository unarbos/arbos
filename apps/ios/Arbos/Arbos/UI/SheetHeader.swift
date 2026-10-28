import SwiftUI

// MARK: - Layout ([feedback iphone-add-members-2026-10-26])

/// The Cancel · title · Add bar at the top of a sheet, as rects in bar space.
///
/// The system inline navigation bar gave the title the room and squeezed the trailing
/// control: on "Add members to Arbonites" the Add pill became "A…" and its hit area went
/// with it (Jacob, 2026-10-26 17:52 CR). Here the two controls always keep their natural
/// width; the title takes what is left, centred on the bar, shrinking to
/// `minimumTitleScale` and then cutting with an ellipsis. `SheetHeaderLayoutTests` pins it
/// at the longest titles and the narrowest phone.
struct SheetHeaderLayout: Equatable {
    /// Bar height: the family's pill / field height.
    static let height: CGFloat = Controls.height
    /// Bar → screen edge, the family's top-bar inset.
    static let inset: CGFloat = Controls.inset
    /// Control → title breathing room.
    static let gap: CGFloat = Controls.gap
    /// The title may shrink to 70 % before it is cut.
    static let minimumTitleScale: CGFloat = 0.7

    var leading: CGRect
    var title: CGRect
    var trailing: CGRect
    /// Factor the title is drawn at (1 = full size).
    var titleScale: CGFloat
    /// True when even the smallest scale does not fit: the title ends in an ellipsis.
    var titleTruncated: Bool

    /// The room a title centred on the bar may use — symmetric around the centre, so a short
    /// Cancel next to a wide "Creating…" never pushes a fitting title off centre.
    static func titleRoom(width: CGFloat, leadingWidth: CGFloat, trailingWidth: CGFloat) -> CGFloat {
        let side = inset + max(leadingWidth, trailingWidth) + gap
        return max(0, width - 2 * side)
    }

    /// Everything between the two controls — what a long title gets (centred in it, like the
    /// system bar does when a title no longer fits around the centre).
    static func betweenControls(width: CGFloat, leadingWidth: CGFloat, trailingWidth: CGFloat) -> CGFloat {
        max(0, width - 2 * inset - leadingWidth - trailingWidth - 2 * gap)
    }

    static func solve(width: CGFloat, leadingWidth: CGFloat, titleWidth: CGFloat, trailingWidth: CGFloat) -> SheetHeaderLayout {
        let centred = titleRoom(width: width, leadingWidth: leadingWidth, trailingWidth: trailingWidth)
        let leading = CGRect(x: inset, y: 0, width: leadingWidth, height: height)
        let trailing = CGRect(x: width - inset - trailingWidth, y: 0, width: trailingWidth, height: height)
        if titleWidth <= centred {
            return SheetHeaderLayout(leading: leading, title: CGRect(x: (width - titleWidth) / 2, y: 0, width: titleWidth, height: height),
                                     trailing: trailing, titleScale: 1, titleTruncated: false)
        }
        let room = betweenControls(width: width, leadingWidth: leadingWidth, trailingWidth: trailingWidth)
        let scale = min(1, max(minimumTitleScale, room / titleWidth))
        let drawn = min(room, titleWidth * scale)
        let roomStart = leading.maxX + gap
        return SheetHeaderLayout(leading: leading,
                                 title: CGRect(x: roomStart + (room - drawn) / 2, y: 0, width: drawn, height: height),
                                 trailing: trailing, titleScale: scale, titleTruncated: titleWidth * minimumTitleScale > room)
    }
}

/// The confirm label of the member pickers: `Add` / `Add (2)` while adding to a group,
/// server or secret; `Create` for a new group; the busy forms while the call runs.
enum SheetAction {
    static func label(adding: Bool, busy: Bool, picks: Int) -> String {
        if adding {
            if busy { return "Adding…" }
            return picks > 1 ? "Add (\(picks))" : "Add"
        }
        return busy ? "Creating…" : "Create"
    }

    /// Every label the pickers and Create / Edit sheets can show — the structure test
    /// measures the longest against the longest title on the narrowest phone.
    static let all: [String] = ["Add", "Add (12)", "Adding…", "Create", "Creating…", "Save", "Saving…", "Replace", "Connect", "Done"]
}

// MARK: - View

/// What the bar's trailing control looks like.
enum SheetHeaderActionStyle: Equatable {
    /// Filled `primary` capsule, `Controls.sheetAction` tall (Add, Create, Save on the pickers).
    case pill
    /// Plain semibold text (the Secrets sheets' Save / Replace / Connect).
    case text
}

/// The bar's leading control.
enum SheetHeaderLeading: Equatable {
    /// "Cancel" text — a presented sheet.
    case cancel
    /// A back chevron — a screen pushed inside the sheet's own stack (New group step 2).
    case back
    case none
}

/// Reports a control's measured width so the title's room can be symmetric.
private struct SheetHeaderControlWidthKey: PreferenceKey {
    static var defaultValue: [String: CGFloat] = [:]
    static func reduce(value: inout [String: CGFloat], nextValue: () -> [String: CGFloat]) {
        value.merge(nextValue()) { _, new in new }
    }
}

private extension View {
    func reportSheetHeaderWidth(_ key: String) -> some View {
        background(GeometryReader { geo in
            Color.clear.preference(key: SheetHeaderControlWidthKey.self, value: [key: geo.size.width])
        })
    }
}

/// Cancel · title · action, drawn by the sheet itself in place of the system navigation
/// bar. Both controls are `fixedSize()` — never compressed, never truncated — and the title
/// is laid out by `SheetHeaderLayout`.
struct SheetHeaderBar: View {
    @Environment(\.theme) private var theme
    let title: String
    var leading: SheetHeaderLeading = .cancel
    var onLeading: () -> Void = {}
    /// The action label — nil hides the control (Cancel-only sheets).
    var action: String? = nil
    var actionStyle: SheetHeaderActionStyle = .pill
    var actionEnabled: Bool = true
    var onAction: () -> Void = {}
    @State private var widths: [String: CGFloat] = [:]

    private var titleText: some View {
        Text(title)
            .font(theme.font(Theme.body, weight: .semibold))
            .foregroundStyle(theme.text)
            .lineLimit(1)
    }

    var body: some View {
        GeometryReader { geo in
            let leadingW = widths["leading"] ?? 0
            let trailingW = widths["trailing"] ?? 0
            let titleW = widths["title"] ?? 0
            // A title that fits around the bar's centre is centred there: both control slots take
            // the wider control's width. A longer one gets everything between the controls.
            let centred = titleW <= SheetHeaderLayout.titleRoom(width: geo.size.width, leadingWidth: leadingW, trailingWidth: trailingW)
            let slot = centred ? max(leadingW, trailingW) : 0
            HStack(spacing: SheetHeaderLayout.gap) {
                leadingControl
                    .fixedSize()
                    .layoutPriority(1)
                    .reportSheetHeaderWidth("leading")
                    .frame(minWidth: slot, alignment: .leading)
                titleText
                    .minimumScaleFactor(SheetHeaderLayout.minimumTitleScale)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity)
                    .background(titleText.fixedSize().hidden().reportSheetHeaderWidth("title"))
                    .accessibilityIdentifier("sheet-title")
                    .accessibilityAddTraits(.isHeader)
                trailingControl
                    .fixedSize()
                    .layoutPriority(1)
                    .reportSheetHeaderWidth("trailing")
                    .frame(minWidth: slot, alignment: .trailing)
            }
            .padding(.horizontal, SheetHeaderLayout.inset)
            .frame(width: geo.size.width, height: SheetHeaderLayout.height)
        }
        .frame(height: SheetHeaderLayout.height)
        .padding(.top, Controls.top)
        .background(theme.bg)
        .overlay(alignment: .bottom) { Rectangle().fill(theme.separator).frame(height: 0.5) }
        .onPreferenceChange(SheetHeaderControlWidthKey.self) { widths = $0 }
    }

    @ViewBuilder
    private var leadingControl: some View {
        switch leading {
        case .cancel:
            Button("Cancel", action: onLeading)
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.text)
                .accessibilityIdentifier("sheet-cancel")
        case .back:
            Button(action: onLeading) {
                Image(systemName: "chevron.left")
                    .font(.system(size: Controls.glyph, weight: .semibold))
                    .foregroundStyle(theme.text)
                    .frame(width: Controls.sheetAction, height: Controls.sheetAction)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Back")
            .accessibilityIdentifier("sheet-back")
        case .none:
            Color.clear.frame(width: 0, height: Controls.sheetAction)
        }
    }

    @ViewBuilder
    private var trailingControl: some View {
        if let action {
            Button(action: onAction) {
                switch actionStyle {
                case .pill:
                    Text(action)
                        .font(theme.font(Theme.small, weight: .semibold))
                        .foregroundStyle(theme.onPrimary)
                        .lineLimit(1)
                        .padding(.horizontal, Controls.textInset)
                        .frame(height: Controls.sheetAction)
                        .background(Capsule().fill(theme.primary))
                        .contentShape(Capsule())
                case .text:
                    Text(action)
                        .font(theme.font(Theme.body, weight: .semibold))
                        .foregroundStyle(theme.text)
                        .lineLimit(1)
                        .frame(height: Controls.sheetAction)
                        .contentShape(Rectangle())
                }
            }
            .buttonStyle(.plain)
            .disabled(!actionEnabled)
            .opacity(actionEnabled ? 1 : 0.45)
            .accessibilityIdentifier("sheet-action")
        } else {
            Color.clear.frame(width: 0, height: Controls.sheetAction)
        }
    }
}

extension View {
    /// Replace the system navigation bar with `SheetHeaderBar` on a sheet screen. The
    /// screen keeps its stack (back swipe, pushes); only the bar is ours.
    func sheetHeader(
        title: String,
        leading: SheetHeaderLeading = .cancel,
        onLeading: @escaping () -> Void = {},
        action: String? = nil,
        actionStyle: SheetHeaderActionStyle = .pill,
        actionEnabled: Bool = true,
        onAction: @escaping () -> Void = {}
    ) -> some View {
        modifier(SheetHeaderModifier(leading: leading) {
            SheetHeaderBar(title: title, leading: leading, onLeading: onLeading, action: action,
                           actionStyle: actionStyle, actionEnabled: actionEnabled, onAction: onAction)
        })
    }
}

/// The bar as a top safe-area inset over the screen, the system bar hidden. A pushed step
/// (`.back`) keeps its swipe-right pop through `backSwipeToPop()` — BackSwipe is the one
/// owner of that gesture; a presented sheet only hides the bar.
private struct SheetHeaderModifier<Bar: View>: ViewModifier {
    let leading: SheetHeaderLeading
    @ViewBuilder let bar: () -> Bar

    func body(content: Content) -> some View {
        switch leading {
        case .back:
            content.safeAreaInset(edge: .top, spacing: 0, content: bar).backSwipeToPop()
        case .cancel, .none:
            content.safeAreaInset(edge: .top, spacing: 0, content: bar)
                .toolbar(.hidden, for: .navigationBar)
                .navigationBarBackButtonHidden(true)
        }
    }
}
