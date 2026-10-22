// Leaf avatars (D92): an agent without a photo shows a cream leaf on an
// autumn-coloured disc — nine shapes × ten colours, deterministic from the
// agent id, with the owner's picks on top. The shapes, palette, index rule
// and motion curves come from arbos-core through ArbosFFI, so the iPhone
// draws the same leaf as the Mac and the relay's `avatar_leaf_effective`.
// People keep their D79 initials.

import ArbosFFI
import SwiftUI

/// Remote flag `features.leaf_avatars` from `GET /me` (default on; the relay's
/// `ARBOS_FEATURE_LEAF_AVATARS=0` is the kill switch). Off → D79 initials.
final class LeafAvatarsFlag: ObservableObject {
    static let shared = LeafAvatarsFlag()
    @Published private(set) var on: Bool = UserDefaults.standard.object(forKey: "features.leaf_avatars") as? Bool ?? true

    func set(_ value: Bool) {
        guard value != on else { return }
        on = value
        UserDefaults.standard.set(value, forKey: "features.leaf_avatars")
    }
}

/// The shared tables, cached per launch.
enum LeafArt {
    static let count = Int(leafCount())
    static let colorCount = Int(leafColorCount())
    static let names: [String] = leafNames()
    static let colorNames: [String] = leafColorNames()
    static let ink = Color(uiColor: UIColor(hexString: leafInkHex()) ?? .white)
    static let scale = CGFloat(leafScale())
    static let colors: [Color] = (0..<colorCount).map { Color(uiColor: UIColor(hexString: leafColorHex(index: UInt8($0))) ?? .gray) }
    private static var paths: [Int: Path] = [:]
    private static var pivots: [Int: UnitPoint] = [:]

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
            if animated {
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
            .fill(LeafArt.ink, style: FillStyle(eoFill: true))
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

/// A server's picture: the leaf picked on its Create screen (D92 amendment), else D79 initials.
struct ServerAvatar: View {
    @ObservedObject private var flag = LeafAvatarsFlag.shared
    let server: Server
    var size: CGFloat = Theme.avatar

    var body: some View {
        if let pick = server.leafPick, flag.on {
            LeafAvatar(pick: pick, size: size)
        } else {
            InitialsAvatar(name: server.name, handle: nil, id: server.id, override: server.avatarColorIndex, size: size)
        }
    }
}

/// Leaf + colour picker, pane-kit style: nine leaves on the current disc
/// colour, ten colour discs, and Automatic. Every tap saves at once so the
/// hero above is the live preview.
struct LeafPicker: View {
    @Environment(\.theme) private var theme
    let project: Project
    let onPick: (_ leaf: Int?, _ color: Int?) -> Void

    private var current: LeafPick { LeafArt.pick(project) }
    private var automatic: Bool { project.avatarLeaf == nil && project.avatarLeafColor == nil }

    private let leafColumns = Array(repeating: GridItem(.flexible(), spacing: 8), count: 5)
    private let colorColumns = Array(repeating: GridItem(.flexible(), spacing: 8), count: 5)

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Leaf").font(.system(size: Theme.tiny)).foregroundStyle(theme.muted)
            LazyVGrid(columns: leafColumns, spacing: 10) {
                ForEach(0..<LeafArt.count, id: \.self) { i in
                    let selected = Int(current.leaf) == i
                    Button { onPick(i, project.avatarLeafColor) } label: {
                        VStack(spacing: 4) {
                            LeafAvatar(pick: LeafPick(leaf: UInt8(i), color: current.color), size: 44)
                                .overlay(Circle().strokeBorder(selected ? theme.text : .clear, lineWidth: 2).frame(width: 50, height: 50))
                                .frame(width: 50, height: 50)
                            Text(LeafArt.names[i]).font(.system(size: Theme.tiny))
                                .foregroundStyle(selected ? theme.text : theme.muted)
                                .lineLimit(1).minimumScaleFactor(0.7)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(LeafArt.names[i] + (selected ? ", selected" : ""))
                }
            }
            Text("Colour").font(.system(size: Theme.tiny)).foregroundStyle(theme.muted).padding(.top, 4)
            LazyVGrid(columns: colorColumns, spacing: 10) {
                ForEach(0..<LeafArt.colorCount, id: \.self) { i in
                    let selected = Int(current.color) == i
                    Button { onPick(project.avatarLeaf, i) } label: {
                        ZStack {
                            Circle().fill(LeafArt.colors[i]).frame(width: 30, height: 30)
                            if selected { Circle().strokeBorder(theme.text, lineWidth: 2).frame(width: 38, height: 38) }
                        }
                        .frame(width: 40, height: 40)
                        .contentShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(LeafArt.colorNames[i] + (selected ? ", selected" : ""))
                }
            }
            HStack {
                Button { onPick(nil, nil) } label: {
                    Text(automatic ? "Automatic" : "Back to automatic")
                        .font(.system(size: Theme.small))
                        .foregroundStyle(automatic ? theme.muted : theme.text)
                        .padding(.horizontal, 10).padding(.vertical, 6)
                        .background(Capsule().fill(theme.field))
                }
                .buttonStyle(.plain)
                .disabled(automatic)
                Spacer()
            }
            Text("Automatic picks the leaf and colour from the agent's id, the same on every device; choose your own to make it yours.")
                .font(.system(size: Theme.tiny)).foregroundStyle(theme.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
