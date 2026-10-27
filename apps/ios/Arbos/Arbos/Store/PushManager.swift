// Notifications: permission (asked once, after sign-in, behind a one-line
// primer), APNs registration (token → POST /devices/{id}/push), the
// foreground/tap delegate (suppressed while that chat is on screen; a tap
// opens the chat), the app-icon badge, and the local fallback the
// `/me/pushes` poll still uses on relays without APNs.
//
// Remote payload: `aps.category == "ARBOS_MESSAGE"`, `chat_id`, `project_id`
// ("people" for DMs), `thread-id` = chat id; the service extension turns it
// into a communication notification with the sender's avatar.

import Foundation
import os
import UIKit
import UserNotifications

private let notifyLog = Logger(subsystem: "com.unarbos.arbos.ios", category: "notify")

@MainActor
final class PushManager: NSObject, ObservableObject, UNUserNotificationCenterDelegate {
    static let shared = PushManager()
    /// Posted with `chat_id` and `project_id` when a notification is tapped.
    static let openChat = Notification.Name("arbos.openChat")
    static let category = "ARBOS_MESSAGE"
    /// Incoming call alert (relay `ARBOS_CALL`); Accept / Decline like WhatsApp's banner.
    static let callCategory = "ARBOS_CALL"
    static let callAcceptAction = "ACCEPT"
    static let callDeclineAction = "DECLINE"

    @Published private(set) var status: UNAuthorizationStatus = .notDetermined
    /// Hex APNs token from the system; re-registered on every launch.
    @Published private(set) var apnsToken: String?
    /// The relay's answer to the registration: false until it holds an APNs key.
    @Published var relayPushEnabled: Bool?
    /// Who to send the token to; set by AppModel once signed in.
    var register: ((String, String) async -> Void)?
    /// The chat on screen while the scene is active; banners for it are swallowed.
    /// Nil when backgrounded so a remote/local alert still shows a banner.
    var visibleChatId: () -> String? = { nil }
    /// D78: an agent's hidden plain chats (not its one chat) never show a banner here.
    var isHiddenChat: (String) -> Bool = { _ in false }
    /// People group muted for a duration (or until unmuted): no banners on this phone.
    var isMutedChat: (String) -> Bool = { _ in false }
    private var registeredToken: String?
    /// chat_id → when a banner was shown (local post or APNs willPresent); swallows
    /// a matching APNs twin that lands a moment later for the same chat. Local
    /// willPresent must still present after postMessage claims this slot.
    private var localBannerAt: [String: CFAbsoluteTime] = [:]
    private static let localBannerDedupe: CFAbsoluteTime = 8

    private let primerKey = "pushPrimerShown"
    var primerShown: Bool {
        get { UserDefaults.standard.bool(forKey: primerKey) }
        set { UserDefaults.standard.set(newValue, forKey: primerKey) }
    }

    func prepare() {
        UNUserNotificationCenter.current().delegate = self
        let messageCategory = UNNotificationCategory(identifier: PushManager.category, actions: [], intentIdentifiers: ["INSendMessageIntent"],
                                                     options: [.customDismissAction])
        let accept = UNNotificationAction(identifier: PushManager.callAcceptAction, title: "Accept", options: [.foreground])
        let decline = UNNotificationAction(identifier: PushManager.callDeclineAction, title: "Decline", options: [.destructive])
        let callCategory = UNNotificationCategory(identifier: PushManager.callCategory, actions: [accept, decline],
                                                  intentIdentifiers: [], options: [.customDismissAction])
        UNUserNotificationCenter.current().setNotificationCategories([messageCategory, callCategory])
        Task { await refreshStatus() }
    }

    func refreshStatus() async {
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        status = settings.authorizationStatus
        if status == .authorized || status == .provisional || status == .ephemeral {
            UIApplication.shared.registerForRemoteNotifications()
        }
    }

    /// The primer should show: signed in, never asked, system still undecided.
    var shouldShowPrimer: Bool { status == .notDetermined && !primerShown }

    /// The system prompt; returns whether the user allowed.
    @discardableResult
    func requestPermission() async -> Bool {
        primerShown = true
        let granted = (try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])) ?? false
        await refreshStatus()
        return granted
    }

    // MARK: APNs token (from the app delegate)

    func didRegister(deviceToken: Data) {
        let hex = deviceToken.map { String(format: "%02x", $0) }.joined()
        apnsToken = hex
        Task { await sendTokenIfNeeded() }
    }

    func didFailToRegister(_ error: Error) {
        #if DEBUG
        print("push: registration failed: \(error.localizedDescription)")
        #endif
    }

    /// Sends the token once per (token, relay) — called after sign-in and on token change.
    func sendTokenIfNeeded(force: Bool = false) async {
        guard let token = apnsToken, let register else { return }
        if !force, registeredToken == token { return }
        registeredToken = token
        await register(token, PushManager.environment)
    }

    func forgetRegistration() { registeredToken = nil }

    /// `development` for Xcode/simulator builds, `production` for TestFlight/App Store
    /// (the exported archive carries aps-environment = production).
    static var environment: String {
        #if DEBUG
        return "development"
        #else
        return "production"
        #endif
    }

    // MARK: Badge

    func setBadge(_ count: Int) {
        UNUserNotificationCenter.current().setBadgeCount(max(0, count)) { _ in }
    }

    /// Drop delivered notifications for a chat once it is read — here, or on another device
    /// of mine (`notifications_cleared`).
    func clearDelivered(chatId: String) {
        let center = UNUserNotificationCenter.current()
        center.getDeliveredNotifications { list in
            let ids = list.filter { ($0.request.content.userInfo["chat_id"] as? String) == chatId || $0.request.content.threadIdentifier == chatId }
                .map(\.request.identifier)
            notifyLog.notice("clear chat=\(chatId, privacy: .public) delivered=\(list.count) removed=\(ids.count)")
            if !ids.isEmpty { center.removeDeliveredNotifications(withIdentifiers: ids) }
        }
    }

    // MARK: Local fallback (relays without APNs)

    /// A message notification with the sender's picture, built like the
    /// service extension builds remote ones.
    func postMessage(id: String, info: [AnyHashable: Any]) async {
        guard let push = MessagePush(userInfo: info) else { return }
        if isMutedChat(push.chatId) || isHiddenChat(push.chatId) { return }
        // APNs already showed a banner for this chat a moment ago — skip the twin.
        if let at = localBannerAt[push.chatId], CFAbsoluteTimeGetCurrent() - at < PushManager.localBannerDedupe {
            return
        }
        localBannerAt[push.chatId] = CFAbsoluteTimeGetCurrent()
        let base = UNMutableNotificationContent()
        base.sound = .default
        base.userInfo = info
        base.threadIdentifier = push.chatId
        let content = await push.makeContent(base: base)
        try? await UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: id, content: content, trigger: nil))
    }

    static func post(id: String, title: String, body: String, chatId: String, projectId: String = "people") {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        content.categoryIdentifier = category
        content.userInfo = ["chat_id": chatId, "project_id": projectId]
        content.threadIdentifier = chatId.isEmpty ? "arbos" : chatId
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: id, content: content, trigger: nil)) { _ in }
    }

    // MARK: Delegate

    // Must stay on the main actor: UIKit restores notification state after this
    // returns, and a nonisolated async method resumes off-main (SIGABRT).
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        let info = notification.request.content.userInfo
        if notification.request.content.categoryIdentifier == PushManager.callCategory {
            // In the foreground the ringing screen takes over; no banner on top of it.
            await MainActor.run { CallManager.shared.handleCallAlert(info) }
            return []
        }
        let chatId = info["chat_id"] as? String
        // Local requests have a nil trigger; remote APNs use UNPushNotificationTrigger.
        // postMessage claims localBannerAt before add(), so only skip that slot for the
        // APNs twin — otherwise the local banner itself is swallowed in the foreground
        // (other chats while one chat is open never notify).
        let isRemote = notification.request.trigger is UNPushNotificationTrigger
        let silent = await MainActor.run { () -> Bool in
            guard let chatId else { return false }
            if chatId == visibleChatId() || isHiddenChat(chatId) || isMutedChat(chatId) { return true }
            // Local `/me/pushes` banner already shown for this chat — skip the APNs twin.
            if isRemote,
               let at = localBannerAt[chatId],
               CFAbsoluteTimeGetCurrent() - at < PushManager.localBannerDedupe {
                return true
            }
            // Claim the slot so a later local postMessage does not double-banner.
            localBannerAt[chatId] = CFAbsoluteTimeGetCurrent()
            return false
        }
        // Alert pushes do not hit didReceiveRemoteNotification while foregrounded;
        // wake the same refresh path so blue circles catch up with the banner.
        if chatId != nil {
            await MainActor.run {
                NotificationCenter.default.post(name: PushManager.remoteReceived, object: nil, userInfo: info)
            }
        }
        return silent ? [] : [.banner, .sound, .list, .badge]
    }

    // Same main-actor rule as willPresent — tap completion must not resume off-main.
    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        let info = response.notification.request.content.userInfo
        if response.notification.request.content.categoryIdentifier == PushManager.callCategory {
            let action = response.actionIdentifier
            if action == PushManager.callDeclineAction {
                await MainActor.run { CallManager.shared.handleCallDecline(info) }
                return
            }
            await MainActor.run { CallManager.shared.handleCallAlert(info) }
            if action == PushManager.callAcceptAction {
                await CallManager.shared.accept()
            }
        }
        let chatId = (info["chat_id"] as? String) ?? ""
        let projectId = (info["project_id"] as? String) ?? "people"
        guard !chatId.isEmpty, response.actionIdentifier != UNNotificationDismissActionIdentifier else { return }
        await MainActor.run {
            NotificationCenter.default.post(name: PushManager.openChat, object: nil, userInfo: ["chat_id": chatId, "project_id": projectId])
        }
    }
}

/// UIKit delegate for the APNs callbacks SwiftUI has no hook for.
final class PushAppDelegate: NSObject, UIApplicationDelegate {
    func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        PushManager.shared.prepare()
        // PushKit wants to be registered at launch; the token is posted once the device is signed in.
        CallManager.shared.prepareVoip()
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        PushManager.shared.didRegister(deviceToken: deviceToken)
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        PushManager.shared.didFailToRegister(error)
    }

    /// Silent / content-available pushes: let the model refresh counts.
    func application(_ application: UIApplication, didReceiveRemoteNotification userInfo: [AnyHashable: Any]) async -> UIBackgroundFetchResult {
        await MainActor.run { NotificationCenter.default.post(name: PushManager.remoteReceived, object: nil, userInfo: userInfo) }
        return .newData
    }
}

extension PushManager {
    static let remoteReceived = Notification.Name("arbos.remoteReceived")
}
