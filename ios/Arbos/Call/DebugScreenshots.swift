#if DEBUG
import UIKit

/// `-shots 1` on the launch line: every few seconds the app renders its
/// own window to `Documents/shots/shot-N.png`. `xcrun devicectl device
/// copy from` can then pull them off a real device, which has no
/// screenshot command of its own.
enum DebugScreenshots {
    static func startIfAsked() {
        guard UserDefaults.standard.bool(forKey: "shots") else { return }
        let interval = max(1, UserDefaults.standard.integer(forKey: "shotEvery"))
        let dir = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("shots", isDirectory: true)
        try? FileManager.default.removeItem(at: dir)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        Task { @MainActor in
            var n = 0
            while true {
                try? await Task.sleep(for: .seconds(interval))
                guard let png = capture()?.pngData() else { continue }
                try? png.write(to: dir.appendingPathComponent(String(format: "shot-%03d.png", n)))
                n += 1
            }
        }
    }

    @MainActor
    private static func capture() -> UIImage? {
        let windows = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap(\.windows)
        guard let window = windows.first(where: \.isKeyWindow) ?? windows.first else { return nil }
        let renderer = UIGraphicsImageRenderer(bounds: window.bounds)
        return renderer.image { _ in
            window.drawHierarchy(in: window.bounds, afterScreenUpdates: true)
        }
    }
}
#endif
