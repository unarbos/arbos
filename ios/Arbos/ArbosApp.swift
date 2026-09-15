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

/// Projects first, a project's chat one tap in, the call one more. On a
/// cold start the phone reopens the project it last had open, the way the
/// desktop comes back to its tabs.
struct RootView: View {
    @EnvironmentObject private var settings: AppSettings
    @State private var path = NavigationPath()
    @State private var restored = false

    var body: some View {
        NavigationStack(path: $path) {
            ProjectsView()
                .navigationDestination(for: KernelTarget.self) { target in
                    ProjectChatView(target: target)
                }
        }
        .tint(ArbosTheme.accent)
        .preferredColorScheme(.dark)
        .onAppear {
            guard !restored else { return }
            restored = true
            if UserDefaults.standard.string(forKey: "kernelTarget") != nil {
                path.append(settings.kernelTarget)
            }
        }
    }
}
