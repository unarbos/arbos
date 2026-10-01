import simd
import SwiftUI

/// A floating e8-style metagraph orb: nodes and edges on a sphere, soft
/// cyan/white lattice on near-black, reacting to voice level and call phase.
/// Inspired by the Bittensor homepage metagraph — not a WebView of it.
struct E8OrbView: View {
    let level: Float
    let phase: CallViewModel.Phase

    @State private var shown: Float = 0
    @State private var spin: Double = 0

    private static let nodes: [SIMD3<Float>] = Self.makeNodes(count: 48)
    private static let edges: [(Int, Int)] = Self.makeEdges(nodes: nodes, k: 3)

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 60)) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            let pulse = sin(t * 2 * .pi / 3.2) * 0.5 + 0.5
            Canvas { ctx, size in
                let center = CGPoint(x: size.width / 2, y: size.height / 2)
                let radius = min(size.width, size.height) * 0.42 * CGFloat(discScale(pulse: pulse))
                let yaw = spin + t * idleSpin
                let pitch = 0.35 + 0.08 * sin(t * 0.4)
                let projected = Self.nodes.map { project($0, yaw: yaw, pitch: pitch, center: center, radius: radius) }

                // Soft halo
                let halo = Path(ellipseIn: CGRect(
                    x: center.x - radius * 1.15,
                    y: center.y - radius * 1.15,
                    width: radius * 2.3,
                    height: radius * 2.3
                ))
                ctx.fill(halo, with: .color(ink.opacity(0.08 + 0.12 * Double(shown))))

                // Edges behind (farther z first is approximate by y)
                for (a, b) in Self.edges {
                    let pa = projected[a]
                    let pb = projected[b]
                    var line = Path()
                    line.move(to: pa.point)
                    line.addLine(to: pb.point)
                    let depth = (pa.depth + pb.depth) * 0.5
                    let alpha = 0.12 + 0.35 * Double(depth) + 0.25 * Double(shown)
                    ctx.stroke(line, with: .color(edgeInk.opacity(alpha)), lineWidth: 0.7 + CGFloat(shown) * 0.6)
                }

                // Nodes
                for p in projected {
                    let r: CGFloat = 1.6 + CGFloat(p.depth) * 2.2 + CGFloat(shown) * 1.4
                    let rect = CGRect(x: p.point.x - r, y: p.point.y - r, width: r * 2, height: r * 2)
                    ctx.fill(Path(ellipseIn: rect), with: .color(ink.opacity(0.35 + 0.55 * Double(p.depth))))
                }
            }
            .onChange(of: context.date) { _, _ in
                shown += (level - shown) * 0.45
            }
        }
        .animation(.easeInOut(duration: 0.35), value: phase)
        .accessibilityElement()
        .accessibilityLabel("Call")
        .accessibilityValue(phase.label)
        .accessibilityHint(phase == .idle ? "Tap to call" : "")
        .accessibilityAddTraits(.isButton)
    }

    private func discScale(pulse: Double) -> Float {
        switch phase {
        case .listening, .speaking: return 0.92 + 0.12 * min(1, max(0, shown))
        case .thinking, .connecting: return 0.92 + 0.05 * Float(pulse)
        case .idle, .unconfigured, .failed: return 0.92
        }
    }

    private var idleSpin: Double {
        switch phase {
        case .listening, .speaking: return 0.55
        case .thinking, .connecting: return 0.35
        case .idle, .unconfigured, .failed: return 0.18
        }
    }

    private var ink: Color {
        switch phase {
        case .listening: return Color(hex: 0xe8f4ff)
        case .speaking: return Color(hex: 0x9ec9e8)
        case .thinking, .connecting: return Color(hex: 0xa8b8c8)
        case .idle, .unconfigured: return Color(hex: 0x6a7a88)
        case .failed: return ArbosTheme.danger
        }
    }

    private var edgeInk: Color {
        switch phase {
        case .speaking: return Color(hex: 0xb8d8f0)
        case .listening: return Color(hex: 0xd0e6f5)
        default: return Color(hex: 0x8aa0b0)
        }
    }

    private struct Projected {
        var point: CGPoint
        var depth: Float // 0…1, nearer is larger
    }

    private func project(_ v: SIMD3<Float>, yaw: Double, pitch: Double, center: CGPoint, radius: CGFloat) -> Projected {
        let cy = Float(cos(yaw)), sy = Float(sin(yaw))
        let cp = Float(cos(pitch)), sp = Float(sin(pitch))
        // yaw then pitch
        let x1 = v.x * cy - v.z * sy
        let z1 = v.x * sy + v.z * cy
        let y2 = v.y * cp - z1 * sp
        let z2 = v.y * sp + z1 * cp
        let depth = (z2 + 1) * 0.5
        let scale = radius * CGFloat(0.85 + 0.25 * depth)
        return Projected(
            point: CGPoint(x: center.x + CGFloat(x1) * scale, y: center.y + CGFloat(y2) * scale),
            depth: depth
        )
    }

    /// Fibonacci sphere points — even coverage that reads as a lattice.
    private static func makeNodes(count: Int) -> [SIMD3<Float>] {
        let golden = Float.pi * (3 - sqrt(5))
        return (0..<count).map { i in
            let y = 1 - (Float(i) / Float(count - 1)) * 2
            let r = sqrt(max(0, 1 - y * y))
            let theta = golden * Float(i)
            return SIMD3(cos(theta) * r, y, sin(theta) * r)
        }
    }

    private static func makeEdges(nodes: [SIMD3<Float>], k: Int) -> [(Int, Int)] {
        var edges: [(Int, Int)] = []
        for i in nodes.indices {
            var nearest: [(Float, Int)] = []
            for j in nodes.indices where j != i {
                let d = simd_distance(nodes[i], nodes[j])
                nearest.append((d, j))
            }
            nearest.sort { $0.0 < $1.0 }
            for (_, j) in nearest.prefix(k) where i < j {
                edges.append((i, j))
            }
        }
        return edges
    }
}
