import SwiftUI
import UIKit

/// The two UIKit callbacks SwiftUI has no modifier for: the APNs token
/// and its refusal. Kept for future push; the orb app does not surface them.
final class PushDelegate: NSObject, UIApplicationDelegate {
    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        Task { @MainActor in Notifier.current?.tokenArrived(deviceToken) }
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        Task { @MainActor in Notifier.current?.registrationFailed(error) }
    }
}

@main
struct ArbosApp: App {
    @UIApplicationDelegateAdaptor(PushDelegate.self) private var pushDelegate
    @StateObject private var settings: AppSettings
    @StateObject private var link: VoiceLink
    @StateObject private var chat: ChatStore
    @StateObject private var notifier = Notifier()

    init() {
        // The navigation bars UIKit draws cannot read SwiftUI's fonts.
        ArbosTheme.adoptAppearance()
        let settings = AppSettings()
        let link = VoiceLink(settings: settings)
        _settings = StateObject(wrappedValue: settings)
        _link = StateObject(wrappedValue: link)
        // ChatStore stays for CallViewModel's pipeline path; the orb UI never shows it.
        _chat = StateObject(wrappedValue: ChatStore(settings: settings, link: link))
    }

    var body: some Scene {
        WindowGroup {
            OrbHomeView(settings: settings, chat: chat, link: link)
                .environmentObject(settings)
                .environmentObject(chat)
                .environmentObject(link)
                .environmentObject(notifier)
                .tint(ArbosTheme.accent)
                // No `preferredColorScheme`: bittensor.com ships both token sets
                // and remembers which you picked, so the phone's setting decides.
        }
    }
}
