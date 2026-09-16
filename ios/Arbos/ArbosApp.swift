import SwiftUI

@main
struct ArbosApp: App {
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
            if phase == .active { chat.resumeIfNeeded() }
        }
        .onAppear {
            // A reply, an ask or a worker's finish while the app is not in
            // front becomes a notification for the project it came from.
            chat.onAttention = { [weak chat, weak notifier] attention in
                guard let chat, let notifier else { return }
                notifier.post(attention, target: chat.settings.kernelTarget.stored)
            }
        }
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
