import CoreText
import SwiftUI
import UIKit

/// bittensor.com's design system, lifted from the site's own stylesheets.
///
/// The site declares two token sets as custom properties — `html` for dark and
/// `html.light` for light — over one shared grey ramp on `:root`. Every colour
/// below is one of those values, named in the comment it came from, so a change
/// on the site is a one-line change here. The page the orb comes from runs the
/// light set (`<html class="light ...">`), but the site ships both and
/// remembers which you chose, so the app follows the phone rather than pinning
/// one of them.
///
/// Type is FiraCode, which is what the site sets for every piece of chrome
/// around the e8 figure: weight 400, 12 px, 3% letter spacing, 150% line
/// height, labels upper case. The body face on the site is Haffer, which is
/// licensed and cannot ship here; FiraCode is SIL OFL and is the face you
/// actually read on the homepage, so it carries the whole app.
enum ArbosTheme {
    // MARK: - Panels

    /// `--background-default`.
    static let bg = dynamic(light: 0xffffff, dark: 0x111111)
    /// `--background-paper`, which is what the site puts behind the page.
    static let surface = dynamic(light: 0xffffff, dark: 0x050404)
    /// `--secondary-main`. White on white in the light set: the site separates
    /// blocks with a hairline, not with a fill.
    static let card = dynamic(light: 0xffffff, dark: 0x1c1c1c)
    /// `--secondary-main`, used where something has to sit above the page.
    static let raised = dynamic(light: 0xe3e3e3, dark: 0x1c1c1c)
    /// `--secondary-light`.
    static let raisedHover = dynamic(light: 0xdadada, dark: 0x222222)
    static let overlay = dynamic(light: 0xffffff, dark: 0x1c1c1c)
    static let inputBg = dynamic(light: 0xffffff, dark: 0x1c1c1c)
    /// `--border-primary`. Full strength, not a washed-out one — the crisp
    /// hairline is most of why the site reads as a schematic.
    static let border = dynamic(light: 0x262626, dark: 0x292929)
    /// `--border-secondary` on dark, `--black` on light.
    static let borderStrong = dynamic(light: 0x000000, dark: 0x3a3a3a)
    /// `--grey-200` / `--secondary-light`.
    static let elementActive = dynamic(light: 0xf1f3f4, dark: 0x222222)
    /// Behind inline code in a reply: `--grey-200` / `--secondary-light`.
    static let codeChip = dynamic(light: 0xf1f3f4, dark: 0x222222)

    // MARK: - Ink

    /// `--text-primary`. Kept as a `UIColor` too, because the navigation bar is
    /// drawn by UIKit and cannot read a SwiftUI colour.
    static let textColor = ui(light: 0x292929, dark: 0xe0e0e0)
    static let text = Color(uiColor: textColor)
    /// `--grey-500` / `--text-secondary`. The light set's own `--text-secondary`
    /// is `#e0e0e0`, which it uses for rules rather than for words; the site's
    /// secondary *text* on a white page is `--text-secondary-kh`, the grey ramp's
    /// 500.
    static let textMuted = dynamic(light: 0x5f6368, dark: 0x767676)
    static let textFaint = dynamic(light: 0x767676, dark: 0x5f6368)
    static let textDim = dynamic(light: 0x767676, dark: 0x767676)
    /// `--primary-main` on light, `--info-main` on dark: the site's accent is a
    /// blue on white and a violet on black.
    static let accent = dynamic(light: 0x2f46f4, dark: 0xa77dff)
    /// `--error-main`.
    static let danger = dynamic(light: 0xd93737, dark: 0xf56868)
    /// `--success-main`.
    static let ok = dynamic(light: 0x49c24e, dark: 0x8ae06c)
    // The site's `--warning-main` is #eeac3c, which is under 2:1 against a white
    // page: it is a badge colour there, not a text one. Left out rather than left
    // lying around for someone to set a sentence in.

    // MARK: - Radii

    /// The site rounds by 5 px and nothing more; corners are all but square.
    static let composerRadius: CGFloat = 5
    static let promptRadius: CGFloat = 5
    static let cardRadius: CGFloat = 5
    static let controlRadius: CGFloat = 4

    // MARK: - Spacing

    /// The site's header pads `24px 22px` on a phone.
    static let gutter: CGFloat = 22
    static let barMargin: CGFloat = 12
    static let promptPadX: CGFloat = 12
    static let promptPadY: CGFloat = 10
    static let rowGap: CGFloat = 6
    static let itemGap: CGFloat = 10
    /// `line-height: 150%`, expressed as the leading SwiftUI adds on top of the
    /// glyphs.
    static let lineSpacing: CGFloat = bodySize * 0.5

    // MARK: - Type

    /// `font-size: 12px !important` on the site's header: the size every label
    /// and status line in this app is set at.
    static let captionSize: CGFloat = 12
    static let monoSize: CGFloat = 12
    static let calloutSize: CGFloat = 14
    static let bodySize: CGFloat = 15
    static let titleSize: CGFloat = 18

    static let title = font(titleSize)
    static let body = font(bodySize)
    static let bodyMedium = font(bodySize, weight: .medium)
    static let bodySemibold = font(bodySize, weight: .medium)
    static let callout = font(calloutSize)
    static let calloutMedium = font(calloutSize, weight: .medium)
    static let caption = font(captionSize)
    static let mono = font(monoSize)

    /// FiraCode at a size, falling back to the system's monospace if the
    /// resource did not make it into the bundle. The fallback matters: a missing
    /// custom font resolves to the proportional system face without complaining,
    /// which would quietly undo the whole look.
    static func font(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        guard let face = Face.named(weight) else {
            return .system(size: size, weight: weight, design: .monospaced)
        }
        // Scaled rather than fixed: the site can set 12 px and stop, but a phone
        // whose owner has turned text up has to be readable at the same size
        // everything else on it is.
        return .custom(face, size: size, relativeTo: .body)
    }

    /// `letter-spacing: 3%`, which the site states as a proportion of the size.
    static func tracking(_ size: CGFloat) -> CGFloat { size * 0.03 }

    /// The tracking that goes with ``caption``, the size most of this app is set
    /// at.
    static let captionTracking = tracking(captionSize)

    /// UIKit draws the navigation bar and the keyboard's own furniture, and
    /// neither reads SwiftUI's fonts. Called once at launch.
    static func adoptAppearance() {
        let appearance = UINavigationBarAppearance()
        appearance.configureWithTransparentBackground()
        appearance.titleTextAttributes = [
            .foregroundColor: textColor,
            .kern: captionTracking,
            .font: uiFont(captionSize),
        ]
        appearance.largeTitleTextAttributes = [
            .foregroundColor: textColor,
            .kern: tracking(titleSize),
            .font: uiFont(titleSize),
        ]
        UINavigationBar.appearance().standardAppearance = appearance
        UINavigationBar.appearance().compactAppearance = appearance
        UINavigationBar.appearance().scrollEdgeAppearance = appearance
    }

    /// The UIKit twin of ``font(_:weight:)``, for the bars SwiftUI does not draw.
    static func uiFont(_ size: CGFloat, weight: UIFont.Weight = .regular) -> UIFont {
        let swiftWeight: Font.Weight = weight == .regular ? .regular : .medium
        guard let face = Face.named(swiftWeight), let custom = UIFont(name: face, size: size) else {
            return .monospacedSystemFont(ofSize: size, weight: weight)
        }
        return custom
    }

    /// The bundled faces, registered with Core Text the first time one is asked
    /// for. Registration is verified by looking the face up afterwards, so this
    /// works whether `UIAppFonts` already took care of it, we did, or the file
    /// is missing altogether.
    private enum Face {
        static func named(_ weight: Font.Weight) -> String? {
            switch weight {
            case .ultraLight, .thin, .light, .regular: return regular
            default: return medium
            }
        }

        private static let regular = register("FiraCode-Regular")
        private static let medium = register("FiraCode-Medium")

        private static func register(_ name: String) -> String? {
            if let url = Bundle.main.url(forResource: name, withExtension: "ttf") {
                CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
            }
            guard UIFont(name: name, size: ArbosTheme.captionSize) != nil else {
                NSLog("Theme: %@ is not in the bundle; falling back to the system monospace.", name)
                return nil
            }
            return name
        }
    }

    /// The site swaps a whole token set on the `html` element; this is the same
    /// swap, resolved per view by the trait the phone is in.
    private static func dynamic(light: UInt32, dark: UInt32) -> Color {
        Color(uiColor: ui(light: light, dark: dark))
    }

    private static func ui(light: UInt32, dark: UInt32) -> UIColor {
        UIColor { traits in UIColor(hex: traits.userInterfaceStyle == .dark ? dark : light) }
    }
}

extension Color {
    init(hex: UInt32, opacity: Double = 1) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xff) / 255,
            green: Double((hex >> 8) & 0xff) / 255,
            blue: Double(hex & 0xff) / 255,
            opacity: opacity
        )
    }
}

extension UIColor {
    convenience init(hex: UInt32) {
        self.init(
            red: CGFloat((hex >> 16) & 0xff) / 255,
            green: CGFloat((hex >> 8) & 0xff) / 255,
            blue: CGFloat(hex & 0xff) / 255,
            alpha: 1
        )
    }
}

extension View {
    /// The site's chrome treatment: FiraCode at 12 px, 3% letter spacing, upper
    /// case — its header links, and every label and status line here.
    func bittensorLabel() -> some View {
        bittensorNote().textCase(.uppercase)
    }

    /// The same, in the case it was written in. Anything that is a sentence
    /// rather than a label goes through here: the site shouts its chrome, not its
    /// prose, and an upper-case apology for a refused microphone reads as a fault
    /// in the app.
    func bittensorNote() -> some View {
        font(ArbosTheme.caption)
            .tracking(ArbosTheme.captionTracking)
            .lineSpacing(ArbosTheme.captionSize * 0.5)
    }
}

/// The site's text link (`.Link_link__9snV0`): a chrome label with the hairline
/// rule underneath it. The site reveals the rule on hover; a phone has no hover,
/// so it is simply there.
struct BittensorLink: View {
    let title: String
    var tint: Color = ArbosTheme.text
    /// False where the title is a sentence the app composed rather than a label
    /// it chose.
    var shout = true
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            label
                .multilineTextAlignment(.center)
                .overlay(alignment: .bottom) {
                    Rectangle().frame(height: 1).offset(y: 3)
                }
                // Outside the overlay so the rule takes the same colour as the
                // words, and tall enough to be hit with a thumb.
                .foregroundStyle(tint)
                .frame(minHeight: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private var label: some View {
        if shout {
            Text(title).bittensorLabel()
        } else {
            Text(title).bittensorNote()
        }
    }
}
