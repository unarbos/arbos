// Chat pictures, decoded once and off the main actor (iphone-scroll-perf release 3,
// 2026-10-28). `ImageBubble` used to run `UIImage(data:)` + `ImageTrim` on the main actor
// inside its `.task`, and — since the transcript rows went lazy (#2149) — did it again every
// time a picture row was realised on a trip through history, showing the 240 × 160
// placeholder until the bytes came back (a layout jump under the reader's finger).
//
// Now: one decode per blob, on a background task, with the bitmap prepared for display
// (`preparingForDisplay`), kept in an `NSCache` by account, blob slot and ref with the picture's natural size
// beside it. A re-realised row paints at its final size at once from the cache; a row whose
// picture is not cached yet still lays out at the remembered size while the bytes load.

import AVFoundation
import Foundation
import UIKit

enum ChatImageCache {
    struct Key: Hashable, Sendable {
        let owner: WorkLifetime.Ticket
        let slot: String
        let ref: String
    }

    private final class WrappedKey: NSObject {
        let value: Key
        init(_ value: Key) { self.value = value }
        override var hash: Int { value.hashValue }
        override func isEqual(_ object: Any?) -> Bool {
            (object as? WrappedKey)?.value == value
        }
    }

    /// What a bubble shows: the picture (all frames for an animated one), the first frame
    /// to hold while the row is off screen, whether it animates.
    struct Decoded: @unchecked Sendable {
        let image: UIImage
        let stillFrame: UIImage?
        let animated: Bool
        /// The picture's own size (after the trim), what the bubble lays out from — the
        /// bitmap itself may be a smaller thumbnail.
        let naturalSize: CGSize
    }

    private static let images: NSCache<WrappedKey, Entry> = {
        let c = NSCache<WrappedKey, Entry>()
        c.totalCostLimit = 48 * 1024 * 1024
        return c
    }()

    private final class Entry {
        let decoded: Decoded
        init(_ decoded: Decoded) { self.decoded = decoded }
    }

    /// Natural sizes outlive the bitmaps (a few bytes each): a picture evicted from the
    /// bitmap cache still lays out at its real size while it reloads.
    @MainActor private static var sizes: [Key: CGSize] = [:]

    @MainActor
    static func decoded(for key: Key) -> Decoded? {
        images.object(forKey: WrappedKey(key))?.decoded
    }

    @MainActor
    static func naturalSize(for key: Key) -> CGSize? {
        if let hit = images.object(forKey: WrappedKey(key)) { return hit.decoded.naturalSize }
        return sizes[key]
    }

    @MainActor
    static func store(_ decoded: Decoded, for key: Key) {
        let cost = Int(decoded.image.size.width * decoded.image.scale * decoded.image.size.height * decoded.image.scale * 4)
            * max(1, decoded.image.images?.count ?? 1)
        images.setObject(Entry(decoded), forKey: WrappedKey(key), cost: cost)
        sizes[key] = decoded.naturalSize
    }

    /// Decode the bytes away from the main actor. Still pictures are trimmed (`ImageTrim`)
    /// and reduced to a display-ready thumbnail no larger than `maxPixels` (the bubble's
    /// largest tile at the screen's scale — the viewer loads the full picture itself), so the
    /// first draw does no decoding and the cache holds bubble-sized bitmaps; animated GIF /
    /// WebP keep every frame (`AnimatedImageFrames`). `nil` when the bytes are not a picture.
    static func decode(_ data: Data, maxPixels: CGSize) async -> Decoded? {
        await Task.detached(priority: .userInitiated) { () -> Decoded? in
            if let animated = AnimatedImageFrames.animatedImage(from: data) {
                return Decoded(image: animated, stillFrame: animated.images?.first ?? animated, animated: true,
                               naturalSize: animated.size)
            }
            guard let img = UIImage(data: data) else { return nil }
            let trimmed = ImageTrim.trimmed(img)
            let natural = trimmed.size
            let px = CGSize(width: natural.width * trimmed.scale, height: natural.height * trimmed.scale)
            let ready: UIImage
            if px.width > maxPixels.width || px.height > maxPixels.height {
                let k = min(maxPixels.width / px.width, maxPixels.height / px.height)
                let target = CGSize(width: (px.width * k).rounded(), height: (px.height * k).rounded())
                ready = trimmed.preparingThumbnail(of: target) ?? trimmed.preparingForDisplay() ?? trimmed
            } else {
                ready = trimmed.preparingForDisplay() ?? trimmed
            }
            return Decoded(image: ready, stillFrame: nil, animated: false, naturalSize: natural)
        }.value
    }
}

/// A video's poster frame, made on the device (tester report, 2026-11-17: "a preview of a
/// sent video"). The first decodable frame, no larger than the bubble, kept in
/// `ChatImageCache` under the video's own ref so it is made once.
///
/// Known ceiling: a transcript row has to fetch the whole video to make its poster (the relay
/// caps a blob at 64 MB), once per video per launch. A poster the sender uploads with the
/// message (unarbos/arbos#2983) removes the fetch; this stays as the fallback for rows without one.
enum VideoThumb {
    struct Frame: @unchecked Sendable {
        let image: UIImage
        let seconds: Double
    }

    /// The poster's place in the picture cache: beside, never instead of, the bytes at `blobRef`.
    static func cacheRef(_ blobRef: String) -> String { "poster:" + blobRef }

    /// Which rows try for a poster: a video stored on the relay. One still uploading
    /// (`local:`) or with no ref has no bytes to read and keeps the file card.
    static func wantsPoster(kind: MediaItem.Kind, blobRef: String?) -> Bool {
        guard kind == .video, let blobRef, !blobRef.isEmpty else { return false }
        return !blobRef.hasPrefix("local:")
    }

    @MainActor private static var durations: [ChatImageCache.Key: Double] = [:]
    /// Bytes that are not a video AVFoundation can read (WebM, a mislabelled file): not
    /// fetched again on every trip through history.
    @MainActor static var undecodable: Set<ChatImageCache.Key> = []

    @MainActor
    static func store(_ frame: Frame, for key: ChatImageCache.Key) {
        ChatImageCache.store(.init(image: frame.image, stillFrame: nil, animated: false, naturalSize: frame.image.size), for: key)
        durations[key] = frame.seconds
    }

    @MainActor
    static func seconds(for key: ChatImageCache.Key) -> Double { durations[key] ?? 0 }

    /// `0:07`, `1:05`, `1:05:03`; empty when the length is unknown. The file is untrusted:
    /// anything not finite and positive has no label, and the hours are capped.
    static func durationLabel(_ seconds: Double) -> String {
        guard seconds.isFinite, seconds >= 0.5 else { return "" }
        let total = Int(min(seconds.rounded(), 359_999))
        let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60)
        return h > 0 ? String(format: "%d:%02d:%02d", h, m, s) : String(format: "%d:%02d", m, s)
    }

    /// Decode off the main actor. `nil` when the bytes hold no frame AVFoundation can read,
    /// whatever the name or mime says; the caller then keeps what it drew before.
    static func frame(from data: Data, name: String, maxPixels: CGSize) async -> Frame? {
        await Task.detached(priority: .utility) { () -> Frame? in
            // AVFoundation reads a file and takes a hint from its extension. The name is the
            // sender's, so only a known extension is kept and the file gets its own name.
            let ext = (name as NSString).pathExtension.lowercased()
            let file = FileManager.default.temporaryDirectory
                .appendingPathComponent("poster-\(UUID().uuidString).\(["mov", "m4v"].contains(ext) ? ext : "mp4")")
            guard (try? data.write(to: file)) != nil else { return nil }
            defer { try? FileManager.default.removeItem(at: file) }
            let asset = AVURLAsset(url: file)
            let generator = AVAssetImageGenerator(asset: asset)
            generator.appliesPreferredTrackTransform = true
            generator.maximumSize = maxPixels
            guard let frame = try? await generator.image(at: .zero).image else { return nil }
            let seconds = (try? await asset.load(.duration)).map(CMTimeGetSeconds) ?? 0
            return Frame(image: UIImage(cgImage: frame), seconds: seconds)
        }.value
    }
}
