import MetalKit
import SwiftUI
import simd

/// The floating e8 metagraph: the figure from bittensor.com, drawn with the
/// same roots, the same edges and the same tumbling projection (see
/// `E8Lattice`). The site's WebGL program is reproduced below in Metal — two
/// inks chosen per vertex, alpha blended, one GL_LINES pass over 6720 edges —
/// and the inks are the site's own `vec4(1, 1, 1, 0.9)` and
/// `vec4(0, 0, 0, 0.9)`, over `ArbosTheme.bg`. Half the vertices are therefore
/// the colour of the page and the lines fade into it, which is the look the
/// site has and the reason the figure seems to float. It comes out right in both
/// of bittensor's themes without changing anything: on white the black half
/// carries the figure, on `#111` the white half does.
///
/// The figure tumbles only while there is a call on the line. Off the call it
/// holds the pose it stopped in — still, scale 1, alpha 1, which is the site's
/// frame frozen. So the motion is the call: the figure moving means it is
/// listening to you, and the voice on the line breathes it.
struct E8OrbView: View {
    let level: Float
    let phase: CallViewModel.Phase

    var body: some View {
        E8MetalView(level: level, turning: phase.inCall, presence: presence)
            .allowsHitTesting(false)
            // A drawing, and not the thing you tap: the target that carries the
            // gestures carries the label too, in `OrbHomeView`. Announcing it
            // here as a button put the words on an element with no action behind
            // it, so VoiceOver offered a call it could not place.
            .accessibilityHidden(true)
    }

    /// The alpha the two inks are multiplied by. 1 is the site exactly; the
    /// states below it are the ones where tapping the figure will not get you a
    /// call, which standing still can no longer say on its own.
    private var presence: Float {
        switch phase {
        case .idle, .listening, .speaking, .thinking, .connecting: return 1
        case .failed: return 0.7
        case .unconfigured: return 0.5
        }
    }
}

/// `MTKView` around the ported program. One lattice and one drifting plane per
/// view; positions are recomputed on the CPU each frame exactly as the sketch
/// does, because 240 roots is nothing and it keeps the port honest.
private struct E8MetalView: UIViewRepresentable {
    var level: Float
    var turning: Bool
    var presence: Float

    func makeCoordinator() -> Renderer { Renderer() }

    func makeUIView(context: Context) -> MTKView {
        let view = MTKView(frame: .zero, device: MTLCreateSystemDefaultDevice())
        view.isOpaque = false
        view.backgroundColor = .clear
        view.layer.isOpaque = false
        view.clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        view.enableSetNeedsDisplay = false
        view.isPaused = false
        // The sketch throttles itself to 60 Hz; matching it keeps the drift at
        // the speed it was tuned for on a 120 Hz display.
        view.preferredFramesPerSecond = 60
        view.framebufferOnly = true
        // A device this app cannot draw on leaves a blank view rather than
        // taking the process down: the orb is the whole screen, and a crash
        // here would mean no way to reach Settings.
        guard context.coordinator.attach(to: view) else { return view }
        view.delegate = context.coordinator
        return view
    }

    func updateUIView(_ view: MTKView, context: Context) {
        context.coordinator.level = level
        context.coordinator.turning = turning
        context.coordinator.presence = presence
        // Nothing moves while there is no call, so there is no reason to redraw
        // the same 6720 lines sixty times a second. Lowered rather than paused:
        // a paused `MTKView` that never gets told to resume is a blank screen,
        // and this view is the whole app.
        view.preferredFramesPerSecond = turning ? 60 : 20
    }

    static func dismantleUIView(_ view: MTKView, coordinator: Renderer) {
        view.delegate = nil
        view.isPaused = true
    }

    /// The site's shader pair, translated. Its vertex stage declares `col` as a
    /// `vec2` but binds a single float to it, so `col.y` is always 0 and the test
    /// it writes as `col == vec2(0.1, 0)` is really `shade == 0.1`; `w = 0.9` is
    /// the scale it draws the figure at. The two inks are verbatim. `scale` and
    /// `alpha` are this app's, and at 1 they are both no-ops.
    private static let shaderSource = """
    #include <metal_stdlib>
    using namespace metal;

    struct Uniforms {
        float scale;
        float alpha;
    };

    struct Vertex {
        float4 position [[position]];
        float4 colour;
    };

    vertex Vertex e8_vertex(uint id [[vertex_id]],
                            const device float2 *points [[buffer(0)]],
                            const device float *shades [[buffer(1)]],
                            constant Uniforms &uniforms [[buffer(2)]]) {
        Vertex out;
        out.position = float4(points[id] * uniforms.scale, 0.9, 0.9);
        out.colour = shades[id] == 0.1f ? float4(1, 1, 1, 0.9) : float4(0, 0, 0, 0.9);
        out.colour.a *= uniforms.alpha;
        return out;
    }

    fragment float4 e8_fragment(Vertex in [[stage_in]]) {
        return in.colour;
    }
    """

    /// Mirrors the layout the shader reads.
    private struct Uniforms {
        var scale: Float
        var alpha: Float
    }

    @MainActor
    final class Renderer: NSObject, MTKViewDelegate {
        var level: Float = 0
        var turning = false
        var presence: Float = 1

        private let lattice = E8Lattice.shared
        private var projection = E8Projection()
        private var points: [SIMD2<Float>]
        private var smoothedLevel: Float = 0
        /// Chased rather than set, so a state change fades the figure instead of
        /// stepping it.
        private var smoothedPresence: Float = 1
        private var lastAdvance = CACurrentMediaTime()

        private var queue: MTLCommandQueue?
        private var pipeline: MTLRenderPipelineState?
        private var shadeBuffer: MTLBuffer?
        private var indexBuffer: MTLBuffer?
        /// Three position buffers in rotation: the GPU may still be reading
        /// last frame's while the CPU writes this one.
        private var pointBuffers: [MTLBuffer] = []
        private var frame = 0

        override init() {
            points = Array(repeating: .zero, count: lattice.rootCount)
            super.init()
            projection.project(lattice, into: &points)
        }

        func attach(to view: MTKView) -> Bool {
            guard let device = view.device, let queue = device.makeCommandQueue() else { return false }
            self.queue = queue

            let library: MTLLibrary
            do {
                library = try device.makeLibrary(source: E8MetalView.shaderSource, options: nil)
            } catch {
                NSLog("E8 orb: shader compile failed: %@", String(describing: error))
                return false
            }
            guard let vertexFunction = library.makeFunction(name: "e8_vertex"),
                  let fragmentFunction = library.makeFunction(name: "e8_fragment") else { return false }

            let descriptor = MTLRenderPipelineDescriptor()
            descriptor.vertexFunction = vertexFunction
            descriptor.fragmentFunction = fragmentFunction
            descriptor.colorAttachments[0].pixelFormat = view.colorPixelFormat
            // The site's blendFunc, so overlapping edges build up the same way.
            descriptor.colorAttachments[0].isBlendingEnabled = true
            descriptor.colorAttachments[0].rgbBlendOperation = .add
            descriptor.colorAttachments[0].alphaBlendOperation = .add
            descriptor.colorAttachments[0].sourceRGBBlendFactor = .sourceAlpha
            descriptor.colorAttachments[0].sourceAlphaBlendFactor = .sourceAlpha
            descriptor.colorAttachments[0].destinationRGBBlendFactor = .oneMinusSourceAlpha
            descriptor.colorAttachments[0].destinationAlphaBlendFactor = .oneMinusSourceAlpha
            do {
                pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
            } catch {
                NSLog("E8 orb: pipeline failed: %@", String(describing: error))
                return false
            }

            let pointBytes = lattice.rootCount * MemoryLayout<SIMD2<Float>>.stride
            pointBuffers = (0..<3).compactMap { _ in
                device.makeBuffer(length: pointBytes, options: .storageModeShared)
            }
            shadeBuffer = lattice.shades.withUnsafeBytes {
                device.makeBuffer(bytes: $0.baseAddress!, length: $0.count, options: .storageModeShared)
            }
            indexBuffer = lattice.lineIndices.withUnsafeBytes {
                device.makeBuffer(bytes: $0.baseAddress!, length: $0.count, options: .storageModeShared)
            }
            return pointBuffers.count == 3 && shadeBuffer != nil && indexBuffer != nil
        }

        func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {}

        func draw(in view: MTKView) {
            guard let queue, let pipeline, let shadeBuffer, let indexBuffer,
                  !pointBuffers.isEmpty,
                  let descriptor = view.currentRenderPassDescriptor,
                  let drawable = view.currentDrawable,
                  let buffer = queue.makeCommandBuffer(),
                  let encoder = buffer.makeRenderCommandEncoder(descriptor: descriptor) else { return }

            // One step per 1/60 s of wall clock, as the sketch gates itself, so
            // a dropped frame does not slow the drift down. Off the call there
            // are no steps at all: the plane stops drifting and the figure holds
            // the pose it was in.
            let now = CACurrentMediaTime()
            if turning {
                var steps = Int((now - lastAdvance) * 60)
                if steps > 0 {
                    lastAdvance = now
                    // A view that was off screen for a while must not fast-forward
                    // through thousands of steps on its first frame back.
                    steps = min(steps, 4)
                    for _ in 0..<steps { projection.advance() }
                    projection.project(lattice, into: &points)
                }
            } else {
                // Kept current while still, so the first frame of the next call
                // advances by one step rather than by however long it stood there.
                lastAdvance = now
            }

            smoothedLevel += (min(1, max(0, level)) - smoothedLevel) * 0.18
            smoothedPresence += (presence - smoothedPresence) * 0.08
            let positions = pointBuffers[frame % pointBuffers.count]
            frame += 1
            points.withUnsafeBytes { bytes in
                positions.contents().copyMemory(from: bytes.baseAddress!, byteCount: bytes.count)
            }

            var uniforms = Uniforms(
                scale: 1 + 0.05 * smoothedLevel,
                alpha: smoothedPresence
            )
            encoder.setRenderPipelineState(pipeline)
            encoder.setVertexBuffer(positions, offset: 0, index: 0)
            encoder.setVertexBuffer(shadeBuffer, offset: 0, index: 1)
            encoder.setVertexBytes(&uniforms, length: MemoryLayout<Uniforms>.stride, index: 2)
            encoder.drawIndexedPrimitives(
                type: .line,
                indexCount: lattice.lineIndices.count,
                indexType: .uint16,
                indexBuffer: indexBuffer,
                indexBufferOffset: 0
            )
            encoder.endEncoding()
            buffer.present(drawable)
            buffer.commit()
        }
    }
}
