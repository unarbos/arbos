#if DEBUG
import notify
import UIKit

/// Rig-only triggers for things the simulator cannot do by itself. `simctl io screenshot`
/// saves a picture of the screen but never posts `UIApplication.userDidTakeScreenshotNotification`,
/// so a rig could not reproduce what follows a real screenshot on a phone (the feedback sheet,
/// its dismissal, the transcript under it — [feedback iphone-chat-empties-after-screenshot-2026-10-28]).
///
/// Each trigger is a Darwin notification name. Post one from the UI-test runner (same
/// simulator) with `notify_post`, or from the Mac with
/// `xcrun simctl spawn <udid> notifyutil -p com.unarbos.arbos.rig.screenshot`.
/// Debug builds only; TestFlight carries none of this.
enum RigTriggers {
    /// The app behaves as if the user just took a system screenshot.
    static let screenshot = "com.unarbos.arbos.rig.screenshot"

    private static var tokens: [Int32] = []
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        register(screenshot) {
            NotificationCenter.default.post(name: UIApplication.userDidTakeScreenshotNotification, object: UIApplication.shared)
        }
    }

    private static func register(_ name: String, _ fire: @escaping @MainActor () -> Void) {
        var token: Int32 = 0
        let status = notify_register_dispatch(name, &token, .main) { _ in
            MainActor.assumeIsolated { fire() }
        }
        if status == NOTIFY_STATUS_OK { tokens.append(token) }
    }
}
#endif
