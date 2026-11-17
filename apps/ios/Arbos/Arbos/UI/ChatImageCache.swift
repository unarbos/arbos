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
/// sent video"): the first decodable frame, no larger than the bubble.
///
/// Only from bytes that are already on the phone: a video being sent, or one fetched to be
/// played. A row never fetches a video to draw its poster; without one it stays the file
/// card. A poster the sender uploads with the message (unarbos/arbos#2983) covers the rest.
enum VideoThumb {
    struct Frame: @unchecked Sendable {
        let image: UIImage
        let seconds: Double
    }

    /// The bubble's largest tile at 2×: at most ~1.4 MB a poster.
    static let maxPixels = CGSize(width: 560, height: 640)
    /// Posters kept at once. Simple ceiling: at the limit the store is emptied and those rows
    /// go back to the file card; an LRU if that ever shows.
    static let limit = 24

    /// Its own store, not `ChatImageCache`'s: a poster is never an answer for a blob ref, and
    /// it is not purged behind a row that is still on screen.
    @MainActor private static var frames: [ChatImageCache.Key: Frame] = [:]
    @MainActor private static var making: [ChatImageCache.Key: Task<Void, Never>] = [:]

    /// Which attachments can have a poster: a video stored on the relay. One still uploading
    /// (`local:`) or with no ref has nothing to be keyed by and keeps the file card.
    static func key(owner: WorkLifetime.Ticket, slot: String, kind: MediaItem.Kind, blobRef: String?) -> ChatImageCache.Key? {
        guard kind == .video, let blobRef, !blobRef.isEmpty, !blobRef.hasPrefix("local:") else { return nil }
        return ChatImageCache.Key(owner: owner, slot: slot, ref: blobRef)
    }

    @MainActor
    static func frame(for key: ChatImageCache.Key) -> Frame? { frames[key] }

    /// The video's bytes are in hand: make its poster once, off the main actor.
    @MainActor
    static func remember(_ data: Data, name: String, for key: ChatImageCache.Key) {
        guard frames[key] == nil, making[key] == nil else { return }
        making[key] = Task {
            if let made = await frame(from: data, name: name, maxPixels: maxPixels) {
                if frames.count >= limit { frames.removeAll() }
                frames[key] = made
            }
            making[key] = nil
        }
    }

    /// Returns once a poster being made for `key` is in (or is not coming).
    @MainActor
    static func settled(_ key: ChatImageCache.Key) async { await making[key]?.value }

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
