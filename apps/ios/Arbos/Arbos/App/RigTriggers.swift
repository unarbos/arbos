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
@MainActor
enum RigTriggers {
    /// The app behaves as if the user just took a system screenshot.
    static let screenshot = "com.unarbos.arbos.rig.screenshot"
    /// Someone else starts / stops typing in the chat on screen (the presence row over the
    /// composer appears / goes — a bottom-inset change while a sheet may cover the transcript).
    static let typingOn = "com.unarbos.arbos.rig.typing-on"
    static let typingOff = "com.unarbos.arbos.rig.typing-off"

    private static var tokens: [Int32] = []
    private static var installed = false

    static func install(model: AppModel) {
        guard !installed else { return }
        installed = true
        register(screenshot) {
            NotificationCenter.default.post(name: UIApplication.userDidTakeScreenshotNotification, object: UIApplication.shared)
        }
        register(typingOn) { [weak model] in model?.rigTyping(true) }
        register(typingOff) { [weak model] in model?.rigTyping(false) }
    }

    private static func register(_ name: String, _ fire: @escaping @MainActor () -> Void) {
        var token: Int32 = 0
        let status = notify_register_dispatch(name, &token, .main) { _ in
            MainActor.assumeIsolated { fire() }
        }
        if status == NOTIFY_STATUS_OK { tokens.append(token) }
    }
}

extension AppModel {
    /// A rig peer types (or stops) in the chat on screen: the first other person of that chat,
    /// else a fixed handle.
    fileprivate func rigTyping(_ on: Bool) {
        guard let chatId = visibleChatId else { return }
        let handle = peopleChats.first { $0.id == chatId }?.members.first { !$0.isAgent && $0.member != myUserId && $0.id != "fx-me" }?.handle ?? "rig"
        noteTyping(["handle": handle, "typing": on], chatId: chatId)
    }
}
#endif
