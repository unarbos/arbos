import Foundation
import UIKit
import UserNotifications

/// What the chat wants the user to know when they are not looking at it.
enum Attention {
    /// The agent's reply for a turn, settled.
    case reply(project: String, text: String)
    /// The agent asked something and waits.
    case ask(project: String, question: String)
    /// A worker finished; its last words.
    case workerDone(project: String, worker: String, words: String)
}

/// Local notifications while the app is not in front: a reply, an ask, a
/// worker's finish. iOS suspends a backgrounded app within seconds unless
/// a call keeps it awake, so this covers the switch-away-and-back case and
/// the call; the phone-asleep-for-hours case needs a push from the hub
/// (asked for in the project store's features inbox).
@MainActor
final class Notifier: NSObject, ObservableObject, UNUserNotificationCenterDelegate {
    /// The project a tapped notification asks the app to open.
    @Published var openTarget: String?
    private var asked = false
    private var hold: UIBackgroundTaskIdentifier = .invalid

    /// The user switched away: keep the socket alive for the half minute
    /// iOS allows, so a reply that is already on its way still rings.
    /// Beyond that the app is suspended and only a push could reach it.
    func holdOpen() {
        release()
        hold = UIApplication.shared.beginBackgroundTask(withName: "arbos.reply") { [weak self] in
            Task { @MainActor in self?.release() }
        }
    }

    /// Back in front (or out of time): let go.
    func release() {
        guard hold != .invalid else { return }
        UIApplication.shared.endBackgroundTask(hold)
        hold = .invalid
    }

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

    /// Post it, unless the app is in front — then the chat itself shows it.
    func post(_ attention: Attention, target: String) {
        guard UIApplication.shared.applicationState != .active else { return }
        let content = UNMutableNotificationContent()
        switch attention {
        case .reply(let project, let text):
            content.title = project
            content.body = Self.firstLine(text)
        case .ask(let project, let question):
            content.title = "\(project) asks"
            content.body = Self.firstLine(question)
            content.interruptionLevel = .timeSensitive
        case .workerDone(let project, let worker, let words):
            content.title = "\(project) · \(worker) done"
            content.body = Self.firstLine(words)
        }
        content.sound = .default
        content.userInfo = ["target": target]
        content.threadIdentifier = target
        let request = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }

    private static func firstLine(_ text: String) -> String {
        let line = text.split(separator: "\n").first.map(String.init) ?? text
        let trimmed = line.trimmingCharacters(in: .whitespaces)
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
