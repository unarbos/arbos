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
    /// Where a push from the hub stands for this phone, in plain words
    /// for Settings. Never pretends a push will arrive when it cannot.
    enum PushState: Equatable {
        /// Not asked yet: pushes wait on the first message the user sends.
        case notAsked
        /// The user said no in the system prompt (or later, in iOS Settings).
        case denied
        /// iOS refused a token: this build has no push entitlement, or no network.
        case noToken(String)
        /// A token exists; the hub has not answered for this project yet.
        case registering
        /// The hub holds the token but has no Apple key yet.
        case hubNoKey
        /// The hub pushes to this phone.
        case on
    }

    /// The project a tapped banner or push asks the app to open.
    @Published var openTarget: String?
    @Published private(set) var pushState: PushState = .notAsked
    /// The APNs token iOS gave this install, as hex; nil until it does.
    @Published private(set) var deviceToken: String?
    /// Set by the app delegate so a token or a failure reaches this object.
    static weak var current: Notifier?
    private var asked = false
    private var hold: UIBackgroundTaskIdentifier = .invalid

    override init() {
        super.init()
        UNUserNotificationCenter.current().delegate = self
        Notifier.current = self
        refreshAuthorization()
    }

    /// The sensible moment: the user has just sent their first message in a
    /// project — now "tell me when it answers" makes sense. Once granted,
    /// iOS is asked for a device token so the hub can push.
    func requestIfNeeded() {
        guard !asked else { return }
        asked = true
        UNUserNotificationCenter.current().getNotificationSettings { [weak self] settings in
            switch settings.authorizationStatus {
            case .notDetermined:
                UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { granted, _ in
                    Task { @MainActor in
                        self?.pushState = granted ? .registering : .denied
                        if granted { UIApplication.shared.registerForRemoteNotifications() }
                    }
                }
            case .denied:
                Task { @MainActor in self?.pushState = .denied }
            default:
                Task { @MainActor in
                    if self?.deviceToken == nil { self?.pushState = .registering }
                    UIApplication.shared.registerForRemoteNotifications()
                }
            }
        }
    }

    /// On launch: a phone that already said yes registers again without a
    /// prompt (tokens rotate; the hub wants the newest).
    private func refreshAuthorization() {
        UNUserNotificationCenter.current().getNotificationSettings { [weak self] settings in
            Task { @MainActor in
                switch settings.authorizationStatus {
                case .authorized, .provisional, .ephemeral:
                    self?.asked = true
                    self?.pushState = .registering
                    UIApplication.shared.registerForRemoteNotifications()
                case .denied:
                    self?.asked = true
                    self?.pushState = .denied
                default:
                    break
                }
            }
        }
    }

    /// iOS gave a device token (from the app delegate).
    func tokenArrived(_ data: Data) {
        deviceToken = data.map { String(format: "%02x", $0) }.joined()
        if pushState == .notAsked || pushState == .denied || { if case .noToken = pushState { return true }; return false }() {
            pushState = .registering
        }
    }

    /// iOS refused a token — typically a build without `aps-environment`.
    func registrationFailed(_ error: Error) {
        deviceToken = nil
        let why = (error as NSError).code == 3000 ? "this build has no push entitlement" : error.localizedDescription
        pushState = .noToken(why)
    }

    /// The hub answered `push`.
    func hubAnswered(enabled: Bool) {
        guard deviceToken != nil else { return }
        pushState = enabled ? .on : .hubNoKey
    }

    /// One line for Settings.
    var pushLine: String {
        switch pushState {
        case .notAsked: return "Off until you send your first message — then iOS asks."
        case .denied: return "Off — allow notifications for Arbos in iOS Settings."
        case .noToken(let why): return "Banners only while the app is open: \(why)."
        case .registering: return "Registering this phone with the hub…"
        case .hubNoKey: return "Banners only while the app is open — the hub has no Apple push key yet."
        case .on: return "On — the hub pushes replies, questions and failures to this phone."
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
