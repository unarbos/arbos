import Foundation
import simd

/// The E8 root system and the tumbling 8→2 projection that draws it, ported
/// from the WebGL sketch behind the metagraph on bittensor.com (the `Canvas`
/// component, webpack module 83578 of the homepage chunk).
///
/// The roots are taken at scale 2, exactly as the sketch builds them: the 112
/// vectors with two entries `±2` and six zeros, plus the 128 all-`±1` vectors
/// with an even number of minus signs. Both families have squared length 8.
/// Sorting them lexicographically fixes the vertex order, and joining every
/// pair whose squared distance is 8 gives the 6720 edges of the Gosset
/// polytope 4_21 — the figure the site draws.
struct E8Lattice {
    static let dimension = 8
    /// The light ink selector. The sketch tags alternating vertices with this
    /// value in a float attribute and branches on it in the vertex shader.
    static let lightInk: Float = 0.1

    /// 240 roots, 8 coordinates each, flat and lexicographically sorted.
    let roots: [Float]
    let rootCount: Int
    /// Two vertex indices per edge: 6720 edges, 13440 entries.
    let lineIndices: [UInt16]
    /// One ink selector per root; `lightInk` picks the light colour.
    let shades: [Float]

    static let shared = E8Lattice()

    init() {
        var vectors: [[Float]] = []
        vectors.reserveCapacity(240)

        // Two entries ±2: four sign combinations for each of the 28 axis pairs.
        for i in 0..<Self.dimension {
            for j in (i + 1)..<Self.dimension {
                for signs in [(false, false), (false, true), (true, false), (true, true)] {
                    var v = [Float](repeating: 0, count: Self.dimension)
                    v[i] = signs.0 ? -2 : 2
                    v[j] = signs.1 ? -2 : 2
                    vectors.append(v)
                }
            }
        }
        // All entries ±1, even number of minus signs: seven free bits and the
        // eighth coordinate carrying their parity.
        for word in 0..<128 {
            var v = [Float](repeating: 0, count: Self.dimension)
            var parity = 0
            for bit in 0..<7 {
                let negative = word & (1 << bit) != 0
                v[bit] = negative ? -1 : 1
                if negative { parity ^= 1 }
            }
            v[7] = parity == 1 ? -1 : 1
            vectors.append(v)
        }
        vectors.sort { a, b in
            for k in 0..<Self.dimension where a[k] != b[k] { return a[k] < b[k] }
            return false
        }

        let count = vectors.count
        rootCount = count
        var flat = [Float](repeating: 0, count: count * Self.dimension)
        for (index, vector) in vectors.enumerated() {
            for k in 0..<Self.dimension { flat[index * Self.dimension + k] = vector[k] }
        }
        roots = flat

        var indices: [UInt16] = []
        indices.reserveCapacity(13_440)
        for a in 0..<count {
            for b in (a + 1)..<count {
                var squared: Float = 0
                for k in 0..<Self.dimension {
                    let d = vectors[a][k] - vectors[b][k]
                    squared += d * d
                }
                if squared == 8 {
                    indices.append(UInt16(a))
                    indices.append(UInt16(b))
                }
            }
        }
        lineIndices = indices

        // The sketch fills a four-wide colour array with a two-wide stride, so
        // each vertex overwrites most of the one before and what survives is
        // the alternation that the shader reads. Reproduced as written rather
        // than tidied, because the alternation is the look.
        var ink = [Float](repeating: 0, count: 4 * count)
        for e in 0..<count {
            ink[2 * e + 0] = 0
            ink[2 * e + 1] = Self.lightInk
            ink[2 * e + 2] = 0.6
            ink[2 * e + 3] = 0.9
        }
        shades = Array(ink.prefix(count))
    }
}

/// The plane the lattice is projected onto, drifting under its own momentum.
///
/// Four 8-vectors: the first two span the viewing plane, the last two are
/// their velocities. Every frame the velocities take a Gaussian step, the
/// plane follows them, and all four are re-orthonormalised — which is what
/// makes the figure tumble through 8-space without ever shearing.
struct E8Projection {
    /// Which earlier vectors each vector is orthogonalised against. Note that
    /// the third is corrected against the first only: that asymmetry is in the
    /// original and removing it changes the motion.
    private static let dependencies: [[Int]] = [[], [0], [0], [0, 1, 2]]
    private static let step: Float = 0.001
    /// The projected coordinates are divided by this before reaching clip
    /// space, which fits all 240 roots inside the viewport.
    private static let spread: Float = 3

    private var basis: [[Float]]
    private var spareGaussian: Float?

    init() {
        basis = (0..<4).map { _ in [Float](repeating: 0, count: E8Lattice.dimension) }
        for k in 0..<E8Lattice.dimension {
            for row in 0..<4 {
                let value = gaussian()
                basis[row][k] = value
            }
        }
        orthonormalise()
    }

    /// One frame of drift: velocities wander, the plane follows, everything is
    /// made orthonormal again.
    mutating func advance() {
        for k in 0..<E8Lattice.dimension {
            // Drawn before the writes: a mutating call inside a compound
            // assignment to `basis` is two overlapping accesses to self.
            let kickX = gaussian()
            let kickY = gaussian()
            basis[0][k] += Self.step * basis[2][k]
            basis[1][k] += Self.step * basis[3][k]
            basis[2][k] += Self.step * kickX
            basis[3][k] += Self.step * kickY
        }
        orthonormalise()
    }

    /// Project every root onto the current plane. `out` must hold one entry
    /// per root.
    func project(_ lattice: E8Lattice, into out: inout [SIMD2<Float>]) {
        let dimension = E8Lattice.dimension
        lattice.roots.withUnsafeBufferPointer { roots in
            basis[0].withUnsafeBufferPointer { x in
                basis[1].withUnsafeBufferPointer { y in
                    for index in 0..<lattice.rootCount {
                        let base = index * dimension
                        var sx: Float = 0
                        var sy: Float = 0
                        for k in 0..<dimension {
                            let value = roots[base + k]
                            sx += x[k] * value
                            sy += y[k] * value
                        }
                        out[index] = SIMD2(sx / Self.spread, sy / Self.spread)
                    }
                }
            }
        }
    }

    private mutating func orthonormalise() {
        for row in 0..<basis.count {
            for other in Self.dependencies[row] {
                var projection: Float = 0
                for k in 0..<E8Lattice.dimension { projection += basis[row][k] * basis[other][k] }
                for k in 0..<E8Lattice.dimension { basis[row][k] -= projection * basis[other][k] }
            }
            var squared: Float = 0
            for k in 0..<E8Lattice.dimension { squared += basis[row][k] * basis[row][k] }
            let length = squared.squareRoot()
            // A vector that collapsed onto the ones before it has no direction
            // left to normalise. Reseeding keeps the figure tumbling instead of
            // filling the buffer with NaN and drawing nothing for good.
            guard length.isFinite, length > 1e-6 else {
                var reseeded = [Float](repeating: 0, count: E8Lattice.dimension)
                for k in 0..<E8Lattice.dimension { reseeded[k] = gaussian() }
                basis[row] = reseeded
                continue
            }
            for k in 0..<E8Lattice.dimension { basis[row][k] /= length }
        }
    }

    /// Box–Muller in pairs, as the sketch does it: one value returned, its
    /// twin kept for the next call.
    private mutating func gaussian() -> Float {
        if let spare = spareGaussian {
            spareGaussian = nil
            return spare
        }
        // Never zero: `log(0)` is −infinity and would poison the whole basis.
        let u1 = Float.random(in: Float.leastNormalMagnitude..<1)
        let u2 = Float.random(in: 0..<1)
        let radius = (-2 * Foundation.log(u1)).squareRoot()
        spareGaussian = radius * cos(2 * .pi * u2)
        return radius * sin(2 * .pi * u2)
    }
}
