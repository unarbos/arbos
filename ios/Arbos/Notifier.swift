import Foundation
import UIKit
import UserNotifications

/// The phone's side of the kernel's `notify` frames (#293). While the app
/// is not in front — the half minute iOS leaves the socket open after a
/// switch-away, or a call in the background — a live `notify` becomes a
/// banner; the app badge is the count of unseen; a `seen` from any client
/// clears both. What the app cannot do alone: reach a phone that iOS has
/// suspended. That needs a push (APNs) from the hub, keyed on the same
/// frame — asked for in the project store's features inbox — and on the
/// way back the replayed `notify`s fill in what the push did not carry.
@MainActor
final class Notifier: NSObject, ObservableObject, UNUserNotificationCenterDelegate {
    /// The project a tapped banner asks the app to open.
    @Published var openTarget: String?
    private var asked = false
    private var hold: UIBackgroundTaskIdentifier = .invalid

    override init() {
        super.init()
        UNUserNotificationCenter.current().delegate = self
    }

    /// Ask once, after the user has a project open — not on the first frame.
    func requestIfNeeded() {
        guard !asked else { return }
        asked = true
        UNUserNotificationCenter.current().getNotificationSettings { settings in
            guard settings.authorizationStatus == .notDetermined else { return }
            UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
        }
    }

    /// The user switched away: keep the socket alive for the half minute
    /// iOS allows, so a reply already on its way still rings. Beyond that
    /// the app is suspended and only a push could reach it.
    func holdOpen() {
        release()
        hold = UIApplication.shared.beginBackgroundTask(withName: "arbos.notify") { [weak self] in
            Task { @MainActor in self?.release() }
        }
    }

    /// Back in front (or out of time): let go.
    func release() {
        guard hold != .invalid else { return }
        UIApplication.shared.endBackgroundTask(hold)
        hold = .invalid
    }

    /// A live `notify`: a banner unless the app is in front (then the
    /// chat itself is the notification), and the badge either way.
    func post(_ notification: KernelNotification, project: String, target: String, unseenCount: Int) {
        setBadge(unseenCount)
        guard UIApplication.shared.applicationState != .active else { return }
        let content = UNMutableNotificationContent()
        content.title = notification.title.isEmpty ? project : "\(project) · \(notification.title)"
        content.body = Self.firstLines(notification.body)
        content.sound = .default
        content.badge = NSNumber(value: unseenCount)
        content.userInfo = ["target": target, "id": notification.id]
        content.threadIdentifier = target
        if notification.isAsk { content.interruptionLevel = .timeSensitive }
        let request = UNNotificationRequest(identifier: "notify-\(target)-\(notification.id)", content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }

    /// `seen {through}` from any client: banners up to it go, the badge
    /// shows what is left.
    func clear(through id: Int, target: String, remaining: Int) {
        let center = UNUserNotificationCenter.current()
        center.getDeliveredNotifications { delivered in
            let gone = delivered
                .filter { ($0.request.content.userInfo["target"] as? String) == target && (($0.request.content.userInfo["id"] as? Int) ?? 0) <= id }
                .map(\.request.identifier)
            if !gone.isEmpty { center.removeDeliveredNotifications(withIdentifiers: gone) }
        }
        setBadge(remaining)
    }

    private func setBadge(_ count: Int) {
        UNUserNotificationCenter.current().setBadgeCount(count) { _ in }
    }

    private static func firstLines(_ text: String) -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.count > 160 ? String(trimmed.prefix(157)) + "…" : trimmed
    }

    // MARK: UNUserNotificationCenterDelegate

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let target = response.notification.request.content.userInfo["target"] as? String
        Task { @MainActor in
            self.openTarget = target
            completionHandler()
        }
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        // In front, the chat is the notification.
        completionHandler([])
    }
}
