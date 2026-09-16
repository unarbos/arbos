import SwiftUI
import UIKit

/// The two UIKit callbacks SwiftUI has no modifier for: the APNs token
/// and its refusal. Both go to the Notifier.
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
    @StateObject private var projects: ProjectStore
    @StateObject private var notifier = Notifier()

    init() {
        let settings = AppSettings()
        let link = VoiceLink(settings: settings)
        _settings = StateObject(wrappedValue: settings)
        _link = StateObject(wrappedValue: link)
        _chat = StateObject(wrappedValue: ChatStore(settings: settings, link: link))
        _projects = StateObject(wrappedValue: ProjectStore(settings: settings))
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(settings)
                .environmentObject(chat)
                .environmentObject(link)
                .environmentObject(projects)
                .environmentObject(notifier)
        }
    }
}

/// Projects first, a project's chat one tap in, the call one more. A cold
/// start lands on the list (Jacob's reference); the list's own composer
/// still talks to the project last open.
struct RootView: View {
    @EnvironmentObject private var chat: ChatStore
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var notifier: Notifier
    @Environment(\.scenePhase) private var scenePhase
    @State private var path = NavigationPath()

    var body: some View {
        NavigationStack(path: $path) {
            ProjectsView(path: $path)
                .navigationDestination(for: KernelTarget.self) { target in
                    ProjectChatView(target: target)
                }
        }
        .tint(ArbosTheme.accent)
        .preferredColorScheme(.dark)
        // A phone that slept for hours comes back with its socket gone:
        // the first thing the returning user sees is the chat as it was,
        // then the replay, not an offline notice.
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .active:
                notifier.release()
                chat.resumeIfNeeded()
            case .background:
                notifier.holdOpen()
            case .inactive:
                break
            @unknown default:
                break
            }
        }
        .onAppear {
            // The kernel's `notify` while the app is not in front becomes a
            // banner for the project it came from; `seen` from any client
            // takes it down again.
            chat.onNotify = { [weak chat, weak notifier] notification in
                guard let chat, let notifier else { return }
                notifier.post(notification, project: chat.title, target: chat.settings.kernelTarget.stored, unseenCount: chat.unseen.count)
            }
            chat.onSeen = { [weak chat, weak notifier] through in
                guard let chat, let notifier else { return }
                notifier.clear(through: through, target: chat.settings.kernelTarget.stored, remaining: chat.unseen.count)
            }
            chat.onPushed = { [weak notifier] enabled in notifier?.hubAnswered(enabled: enabled) }
            chat.pushToken = notifier.deviceToken
        }
        // The token arrives after launch (and rotates): the chat sends it
        // to the hub on the next attach, or now if attached.
        .onChange(of: notifier.deviceToken) { _, token in chat.pushToken = token }
        // A tapped notification lands in that project's chat.
        .onChange(of: notifier.openTarget) { _, stored in
            guard let stored else { return }
            notifier.openTarget = nil
            let target = KernelTarget(stored: stored)
            path = NavigationPath()
            path.append(target)
        }
    }
}

