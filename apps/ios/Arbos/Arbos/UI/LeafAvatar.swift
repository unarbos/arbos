// Leaf avatars (D92): an agent without a photo shows a cream leaf on an
// autumn-coloured disc — nine shapes × ten colours, deterministic from the
// agent id, with the owner's picks on top. The shapes, palette, index rule
// and motion curves come from arbos-core through ArbosFFI, so the iPhone
// draws the same leaf as the Mac and the relay's `avatar_leaf_effective`.
// People keep their D79 initials.

import ArbosFFI
import Lottie
import SwiftUI

/// Remote flag `features.leaf_avatars` from `GET /me` (default on; the relay's
/// `ARBOS_FEATURE_LEAF_AVATARS=0` is the kill switch). Off → D79 initials.
final class LeafAvatarsFlag: ObservableObject {
    static let shared = LeafAvatarsFlag()
    @Published private(set) var on: Bool = UserDefaults.standard.object(forKey: "features.leaf_avatars") as? Bool ?? true
    /// `features.leaf_lottie` (default on): the leaves play their committed Lottie idle / gust
    /// files; off = the native sway.
    @Published private(set) var lottie: Bool = UserDefaults.standard.object(forKey: "features.leaf_lottie") as? Bool ?? true

    func set(_ value: Bool) {
        guard value != on else { return }
        on = value
        UserDefaults.standard.set(value, forKey: "features.leaf_avatars")
    }

    func setLottie(_ value: Bool) {
        guard value != lottie else { return }
        lottie = value
        UserDefaults.standard.set(value, forKey: "features.leaf_lottie")
    }
}

/// One of a leaf's two committed Lottie files.
enum LeafClip {
    case idle, gust
}

/// Jacob's Lottie files, parsed once per leaf and clip (through ArbosFFI's `leafLottieJson`).
enum LeafLottie {
    private static var cache: [String: LottieAnimation] = [:]

    static func animation(leaf: Int, clip: LeafClip) -> LottieAnimation? {
        let key = "\(leaf)-\(clip)"
        if let a = cache[key] { return a }
        let json = leafLottieJson(index: UInt8(leaf), gust: clip == .gust)
        guard let data = json.data(using: .utf8), let a = try? LottieAnimation.from(data: data) else { return nil }
        cache[key] = a
        return a
    }
}

/// A `LottieAnimationView` playing one clip: the idle file loops, the gust plays once and
/// settles on its rest pose. Core Animation drives it (no per-frame SwiftUI work); it pauses
/// when the app goes to the background and when the view leaves the window. The fill is
/// recoloured through the keypath `**.leaf.Color` (the hue's ink, like the static leaf).
struct LeafLottieView: UIViewRepresentable {
    let leaf: Int
    let clip: LeafClip
    /// The leaf's ink: the darker same hue of its disc (v2 rule).
    var color: UIColor

    func makeUIView(context: Context) -> LottieAnimationView {
        let view = LottieAnimationView()
        view.contentMode = .scaleAspectFit
        view.backgroundBehavior = .pauseAndRestore
        view.respectAnimationFrameRate = true
        view.isUserInteractionEnabled = false
        configure(view)
        return view
    }

    func updateUIView(_ view: LottieAnimationView, context: Context) {
        if context.coordinator.key != key { configure(view) }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator { var key = "" }

    private var key: String { "\(leaf)-\(clip)" }

    private func configure(_ view: LottieAnimationView) {
        view.animation = LeafLottie.animation(leaf: leaf, clip: clip)
        var r: CGFloat = 1, g: CGFloat = 1, b: CGFloat = 1, a: CGFloat = 1
        color.getRed(&r, green: &g, blue: &b, alpha: &a)
        view.setValueProvider(ColorValueProvider(LottieColor(r: r, g: g, b: b, a: a)), keypath: AnimationKeypath(keypath: "**.leaf.Color"))
        view.loopMode = clip == .idle ? .loop : .playOnce
        view.currentProgress = 0
        view.play()
    }
}

/// Remote flag `features.voice_mode` from `GET /me` (default **off**; the relay's
/// `ARBOS_FEATURE_VOICE_MODE=1` brings it back without an app release). Off → no voice
/// picker row, no voice catalogue request; the code stays (Jacob, 2026-10-22).
final class VoiceModeFlag: ObservableObject {
    static let shared = VoiceModeFlag()
    @Published private(set) var on: Bool = UserDefaults.standard.object(forKey: "features.voice_mode") as? Bool ?? false

    func set(_ value: Bool) {
        guard value != on else { return }
        on = value
        UserDefaults.standard.set(value, forKey: "features.voice_mode")
    }
}

/// The shared tables, cached per launch.
enum LeafArt {
    static let count = Int(leafCount())
    static let colorCount = Int(leafColorCount())
    static let names: [String] = leafNames()
    static let colorNames: [String] = leafColorNames()
    static let scale = CGFloat(leafScale())
    /// The pastel discs, index order.
    static let colors: [Color] = (0..<colorCount).map { Color(uiColor: UIColor(hexString: leafColorHex(index: UInt8($0))) ?? .gray) }
    /// The leaf inks on those discs (darker, same hue), index order.
    static let inks: [Color] = (0..<colorCount).map { Color(uiColor: UIColor(hexString: leafInkHex(index: UInt8($0))) ?? .white) }
    static let inkUIColors: [UIColor] = (0..<colorCount).map { UIColor(hexString: leafInkHex(index: UInt8($0))) ?? .white }
    static func ink(_ color: UInt8) -> Color { inks[Int(color) % max(colorCount, 1)] }
    private static var paths: [Int: Path] = [:]
    private static var pivots: [Int: UnitPoint] = [:]

    /// "No leaf" (Jacob, 2026-10-25): an entity created from the Create sheet without an uploaded
    /// or generated picture is stored with `avatar_leaf` = `avatar_leaf_color` = 255
    /// (`arbos_core::avatar::leaf::NEUTRAL`) and draws as **the name's first letter on a neutral
    /// grey disc** — never an automatic leaf. Distinct from nil (= the id's automatic leaf), so
    /// every existing agent keeps the leaf it has.
    /// `arbos_core::avatar::leaf::NEUTRAL` (pinned by the core's `leaf::tests` and the relay's
    /// `check_leaf_pick`); the ffi carries no getter so this item stays an apps/ios-only change.
    static let neutral = 255
    static var neutralPick: LeafPick { LeafPick(leaf: UInt8(clamping: neutral), color: UInt8(clamping: neutral)) }
    static func isNeutral(_ leaf: Int?) -> Bool { leaf == neutral }
    /// The one letter a neutral disc carries: the first letter or digit of the name, upper-cased
    /// (`?` for an empty name) — the Mac's `leaf::initial`.
    static func initial(_ name: String) -> String {
        guard let c = name.unicodeScalars.first(where: { CharacterSet.alphanumerics.contains($0) }) else { return "?" }
        return String(c).uppercased()
    }
    /// Neutral disc fill and letter ink per appearance (the picker's "unselected" grey).
    static func neutralColors(dark: Bool) -> (disc: Color, ink: Color) {
        dark
            ? (Color(red: 0x3A / 255, green: 0x3A / 255, blue: 0x3A / 255), Color(white: 0xE5 / 255))
            : (Color(white: 0xE6 / 255), Color(white: 0x4A / 255))
    }

    /// The leaf and colour in effect for a project (owner picks, else the id's cell).
    static func pick(_ project: Project) -> LeafPick {
        leafPick(seed: project.id, leaf: project.avatarLeaf.map { UInt8(clamping: $0) }, color: project.avatarLeafColor.map { UInt8(clamping: $0) })
    }

    static func pick(seed: String, leaf: Int? = nil, color: Int? = nil) -> LeafPick {
        leafPick(seed: seed, leaf: leaf.map { UInt8(clamping: $0) }, color: color.map { UInt8(clamping: $0) })
    }

    /// The outline of shape `index` in a 100 × 100 box (even-odd fill).
    static func path(_ index: Int) -> Path {
        if let p = paths[index] { return p }
        var p = Path()
        for seg in leafSegments(index: UInt8(index)) {
            switch seg {
            case let .moveTo(x, y): p.move(to: CGPoint(x: CGFloat(x), y: CGFloat(y)))
            case let .lineTo(x, y): p.addLine(to: CGPoint(x: CGFloat(x), y: CGFloat(y)))
            case let .cubicTo(x1, y1, x2, y2, x, y):
                p.addCurve(to: CGPoint(x: CGFloat(x), y: CGFloat(y)),
                           control1: CGPoint(x: CGFloat(x1), y: CGFloat(y1)),
                           control2: CGPoint(x: CGFloat(x2), y: CGFloat(y2)))
            case .close: p.closeSubpath()
            }
        }
        paths[index] = p
        return p
    }

    /// The stem pivot of shape `index` as a unit point of the 100 × 100 box.
    static func pivot(_ index: Int) -> UnitPoint {
        if let u = pivots[index] { return u }
        let p = leafPivot(index: UInt8(index))
        let u = UnitPoint(x: CGFloat(p.first ?? 50) / 100, y: CGFloat(p.last ?? 80) / 100)
        pivots[index] = u
        return u
    }

    /// The bubble-bar accent of a leaf agent (its disc colour).
    static func accent(_ project: Project, dark: Bool) -> Color {
        let a = leafAccent(seed: project.id, leaf: project.avatarLeaf.map { UInt8(clamping: $0) }, color: project.avatarLeafColor.map { UInt8(clamping: $0) })
        return Color(uiColor: UIColor(hexString: dark ? a.dark : a.light) ?? .systemGray)
    }
}

/// Shape `index` scaled into the rect (the 100 × 100 box fills it).
struct LeafShape: Shape {
    let index: Int

    func path(in rect: CGRect) -> Path {
        let s = min(rect.width, rect.height) / 100
        let t = CGAffineTransform(translationX: rect.minX, y: rect.minY).scaledBy(x: s, y: s)
        return LeafArt.path(index).applying(t)
    }
}

/// The disc with the leaf. `idle` sways it a few degrees about the stem on
/// the leaf's own loop length (profile heroes only; lists stay still);
/// `gust` is a counter — every change plays the 1 s gust once (new activity,
/// a tap). Both are skipped under Reduce Motion.
struct LeafAvatar: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let pick: LeafPick
    var size: CGFloat = 48
    var idle: Bool = false
    var gust: Int = 0
    @State private var gustStart: Date? = nil

    private var leaf: Int { Int(pick.leaf) }
    private var animated: Bool { !reduceMotion && (idle || gustStart != nil) }

    var body: some View {
        ZStack {
            Circle().fill(LeafArt.colors[Int(pick.color) % max(LeafArt.colorCount, 1)])
            if animated, LeafAvatarsFlag.shared.lottie {
                // The committed Lottie: the gust while one runs (one play, back to rest), else
                // the idle loop; the view is keyed so a new gust restarts the clip.
                LeafLottieView(leaf: leaf, clip: gustStart != nil ? .gust : .idle, color: LeafArt.inkUIColors[Int(pick.color) % max(LeafArt.colorCount, 1)])
                    .id("\(leaf)-\(gustStart?.timeIntervalSinceReferenceDate ?? 0)")
                    .frame(width: size * LeafArt.scale, height: size * LeafArt.scale)
                    .accessibilityHidden(true)
            } else if animated {
                TimelineView(.animation(minimumInterval: 1.0 / 60.0)) { context in
                    glyph.rotationEffect(.radians(Double(angle(at: context.date))), anchor: LeafArt.pivot(leaf))
                }
            } else {
                glyph
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
        .onChange(of: gust) { _, _ in
            guard !reduceMotion else { return }
            gustStart = Date()
        }
        .task(id: gustStart) {
            guard gustStart != nil else { return }
            try? await Task.sleep(nanoseconds: UInt64((Double(leafGustSecs()) + 0.05) * 1_000_000_000))
            if !Task.isCancelled { gustStart = nil }
        }
    }

    private var glyph: some View {
        LeafShape(index: leaf)
            .fill(LeafArt.ink(pick.color), style: FillStyle(eoFill: true))
            .frame(width: size * LeafArt.scale, height: size * LeafArt.scale)
    }

    /// Gust while one is running (it starts and ends at rest), else the idle wave.
    private func angle(at date: Date) -> Float {
        if let start = gustStart {
            let t = Float(date.timeIntervalSince(start))
            if t < leafGustSecs() { return leafGustAngle(t: t) }
        }
        guard idle else { return 0 }
        let t = Float(date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: Double(leafIdleSecs(index: pick.leaf))))
        return leafIdleAngle(index: pick.leaf, t: t)
    }
}

/// The neutral avatar (Jacob, 2026-10-25): the name's first letter, semibold, on a plain grey
/// disc — no gradient, no leaf. An empty name is an empty disc (the Create sheet before a name
/// is typed).
struct NeutralAvatar: View {
    @Environment(\.theme) private var theme
    let name: String
    var size: CGFloat = Theme.avatar

    var body: some View {
        let c = LeafArt.neutralColors(dark: theme.isDark)
        ZStack {
            Circle().fill(c.disc)
            if !name.trimmingCharacters(in: .whitespaces).isEmpty {
                Text(LeafArt.initial(name))
                    .font(.system(size: size * AvatarPalette.initialsScale, weight: .semibold))
                    .foregroundStyle(c.ink)
                    .lineLimit(1)
                    .offset(y: size * 0.01)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
        .accessibilityIdentifier("neutral-avatar")
    }
}

/// A server's picture: the leaf picked on its Create screen (D92 amendment), else D79 initials
/// — or the neutral initial (Jacob, 2026-10-25).
struct ServerAvatar: View {
    @EnvironmentObject private var model: AppModel
    @ObservedObject private var flag = LeafAvatarsFlag.shared
    let server: Server
    var size: CGFloat = Theme.avatar

    var body: some View {
        if let key = server.imageKey, let url = model.blobURL(projectId: "servers:\(server.id)", key: key) {
            // D92 amendment: the uploaded / generated picture wins over the leaf.
            AsyncImage(url: url) { phase in // avatar-lint: allow (server picture, one copy)
                if let image = phase.image {
                    image.resizable().scaledToFill()
                } else {
                    fallback
                }
            }
            .frame(width: size, height: size)
            .clipShape(Circle())
        } else {
            fallback
        }
    }

    @ViewBuilder
    private var fallback: some View {
        if server.isNeutralAvatar {
            NeutralAvatar(name: server.name, size: size)
        } else if let pick = server.leafPick, flag.on {
            LeafAvatar(pick: pick, size: size)
        } else {
            InitialsAvatar(name: server.name, handle: nil, id: server.id, override: server.avatarColorIndex, size: size)
        }
    }
}
