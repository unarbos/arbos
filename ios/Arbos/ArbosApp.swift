import SwiftUI

@main
struct ArbosApp: App {
    @StateObject private var settings: AppSettings
    @StateObject private var link: VoiceLink
    @StateObject private var chat: ChatStore
    @StateObject private var projects: ProjectStore

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
        }
    }
}

/// Projects first, a project's chat one tap in, the call one more. A cold
/// start lands on the list (Jacob's reference); the list's own composer
/// still talks to the project last open.
struct RootView: View {
    @EnvironmentObject private var chat: ChatStore
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
    }
}
