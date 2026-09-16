import UIKit

/// The bytes of photos this phone sent, by stored name, so the card can
/// draw the picture rather than its filename — now, after the kernel's
/// echo, and after a reopen when history replays only the path. Lives in
/// Caches: the system may clear it, and then the card falls back to text.
enum AttachmentCache {
    private static let directory: URL = {
        let base = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        let url = base.appendingPathComponent("attachments", isDirectory: true)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }()

    private static var thumbnails: [String: UIImage] = [:]

    static func store(_ data: Data, as name: String) {
        try? data.write(to: directory.appendingPathComponent(name), options: .atomic)
        thumbnails[name] = nil
    }

    static func has(_ name: String) -> Bool {
        FileManager.default.fileExists(atPath: directory.appendingPathComponent(name).path)
    }

    /// A thumbnail sized for the card, decoded once.
    static func image(_ name: String) -> UIImage? {
        if let cached = thumbnails[name] { return cached }
        guard let data = try? Data(contentsOf: directory.appendingPathComponent(name)), let full = UIImage(data: data) else { return nil }
        let side: CGFloat = 480
        let scale = min(1, side / max(full.size.width, full.size.height))
        let size = CGSize(width: full.size.width * scale, height: full.size.height * scale)
        let small = UIGraphicsImageRenderer(size: size).image { _ in full.draw(in: CGRect(origin: .zero, size: size)) }
        thumbnails[name] = small
        return small
    }
}
