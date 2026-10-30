import SwiftUI

@main
struct ArbosApp: App {
    @UIApplicationDelegateAdaptor(PushAppDelegate.self) private var pushDelegate
    @StateObject private var model = AppModel()
    @Environment(\.colorScheme) private var systemScheme

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(model)
                // The code-card grammars load off the main thread at launch.
                .task { CodeHighlight.warm() }
        }
    }
}

/// An `arbos://join/<code>` or `arbos://join-group/<code>` the platform delivered.
///
/// Nothing about the delivery says who asked for it: a web page, another app, or a link in
/// somebody's message can send the app one of these, and joining is an authenticated act
/// with a lasting consequence — the person lands in a server or group with whoever minted
/// the code, visible to everyone else in it. So the link only ever fills in this question;
/// the join is the Join button.
struct InviteLink: Equatable {
    enum Kind { case server, group }

    let kind: Kind
    let code: String

    init?(url: URL) {
        guard url.scheme == AuthEndpoints.callbackScheme else { return nil }
        switch url.host {
        case "join": kind = .server
        case "join-group": kind = .group
        default: return nil
        }
        let code = url.path.trimmingCharacters(in: CharacterSet(charactersIn: "/ "))
        guard !code.isEmpty else { return nil }
        self.code = code
    }

    static func confirm(_ invite: InviteLink) -> ConfirmSpec {
        let what = invite.kind == .group ? "group" : "server"
        return ConfirmSpec(
            title: "Join this \(what)?",
            message: "A link asked Arbos to join a \(what). Everyone in it will see you joined, so only join if you trust whoever sent the link.",
            action: "Join",
            destructive: false
        )
    }
}

struct RootView: View {
    @EnvironmentObject private var model: AppModel
    @ObservedObject private var push = PushManager.shared
    @ObservedObject private var feedback = FeedbackCenter.shared
    @ObservedObject private var calls = CallManager.shared
    @Environment(\.colorScheme) private var systemScheme
    @Environment(\.scenePhase) private var scenePhase
    @State private var showPrimer = false
    /// An invite link the platform handed us, waiting for its Join.
    @State private var pendingInvite: InviteLink?

    private var theme: Theme {
        Theme.resolve(appearance: model.appearance, surface: model.surface, systemDark: systemScheme == .dark, textScale: model.textScale)
    }

    var body: some View {
        ZStack {
            if model.isPaired {
                MainShellView()
            } else {
                SignInView()
            }
            // D82: first sign-in on a fresh account — the terminal screen sits over
            // the shell until the computer, agent and first chat exist, then morphs
            // into the greeting bubble of the chat underneath.
            if model.isPaired, let firstLogin = model.firstLogin {
                FirstLoginView(state: firstLogin)
                    .zIndex(10)
                    .transition(.asymmetric(insertion: .identity, removal: .opacity))
            }
        }
        .animation(.easeInOut(duration: 0.25), value: model.firstLogin == nil)
        .onAppear {
            #if DEBUG
            PerfLog.shared.startIfRequested()
            PerfLog.shared.observe(model)
            TouchPerf.installIfRequested()
            #endif
            PerfDiagnostics.shared.start(model: model)
            #if DEBUG
            if DebugHooks.perfChanges { setlinebuf(stdout) }
            #endif
            if DebugHooks.show == "onboarding" {
                model.demoFirstLogin(stage: DebugHooks.onboardingStage)
                if DebugHooks.onboardingProfile { model.needsProfileSetup = true }
            }
        }
        .environment(\.theme, theme)
        .preferredColorScheme(preferredScheme)
        .tint(theme.text)
        // Toasts (Jacob 2026-10-26 / 2026-10-27): one slot — under the open chat's header pill
        // (`ChatView` hosts it there); here at the top under the status bar only while no chat
        // is up — an info notice as the plain card, a failure as the shared `ErrorNote`.
        .overlay(alignment: .top) {
            if !model.chatIsUp {
                ToastHost()
                    .environmentObject(model)
                    .environment(\.theme, theme)
            }
        }
        // Fresh account: the name/handle sheet shows over the first-login terminal
        // screen (D82) while the machine boots (8–11 s), as Telegram and Grok ask for
        // the name before the first chat; the notifications primer waits for both.
        .sheet(isPresented: Binding(get: { model.needsProfileSetup },
                                    set: { model.needsProfileSetup = $0 })) {
            ProfileEditView(onboarding: true)
                .environmentObject(model)
                .environment(\.theme, theme)
                .presentationDetents([.height(420), .large])
                .presentationDragIndicator(.hidden)
                .presentationBackground(theme.bg)
                .presentationCornerRadius(28)
                .interactiveDismissDisabled(true)
                .preferredColorScheme(theme.isDark ? .dark : .light)
        }
        .sheet(isPresented: $showPrimer) {
            NotificationsPrimerView().environment(\.theme, theme)
        }
        .sheet(item: $model.profile, onDismiss: { model.profilePath = [] }) { target in
            ProfileSheet(target: target)
                .environmentObject(model)
                .environment(\.theme, theme)
                .presentationDetents([.large])
                .presentationDragIndicator(.hidden)
                .presentationBackground(theme.bg)
                .presentationCornerRadius(28)
                .preferredColorScheme(theme.isDark ? .dark : .light)
        }
        .overlay(alignment: .top) {
            // Collapsed call: the voice pill sits in the chat header while that chat is
            // on screen (ChatView draws it there); everywhere else, and for video, it floats.
            if calls.isActive, !calls.isPresented,
               calls.call?.kind == .video || !model.isShowing(chatId: calls.call?.chatId ?? "") {
                InCallOverlay(expected: expectedPeople)
                    .environmentObject(model)
                    .environment(\.theme, theme)
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
        .animation(.spring(duration: 0.3), value: calls.isPresented)
        .fullScreenCover(isPresented: $calls.isPresented) {
            CallScreen(expected: expectedPeople)
                .environmentObject(model)
                .environment(\.theme, theme)
                .preferredColorScheme(theme.isDark ? .dark : .light)
        }
        .fullScreenCover(isPresented: Binding(get: { calls.incoming != nil && !calls.isPresented }, set: { if !$0 { calls.decline() } })) {
            IncomingCallView()
                .environmentObject(model)
                .environment(\.theme, theme)
                .preferredColorScheme(theme.isDark ? .dark : .light)
        }
        .sheet(isPresented: $feedback.isPresented) {
            FeedbackSheet().environmentObject(model).environment(\.theme, theme)
        }
        // Shake or a system screenshot → Send feedback (TestFlight-style screenshot path).
        .onReceive(NotificationCenter.default.publisher(for: FeedbackCenter.shake)) { _ in
            openFeedbackFromGesture()
        }
        .onReceive(NotificationCenter.default.publisher(for: UIApplication.userDidTakeScreenshotNotification)) { _ in
            openFeedbackFromGesture()
        }
        .onChange(of: model.isPaired, initial: true) { _, paired in schedulePrimer(paired) }
        .onChange(of: model.firstLogin == nil) { _, _ in schedulePrimer(model.isPaired) }
        .onChange(of: model.needsProfileSetup) { _, needed in if !needed { schedulePrimer(model.isPaired) } }
        .onChange(of: push.status) { _, _ in schedulePrimer(model.isPaired) }
        .onChange(of: scenePhase) { _, phase in
            // Match Mac `window_active`: only suppress push while the scene is
            // active. Background / lock clears focused_chat so APNs banners fire.
            // After `.background`, rooms redial so Syncing… cannot stick
            // ([feedback app-01M5MVNNNBZHJWSRCZ81S04C8E]).
            model.noteScenePhase(phase)
            if phase == .active { Task { await push.refreshStatus(); await model.refreshUnread(); await CallManager.shared.sendVoipTokenIfNeeded(); await model.checkForUpdate() } }
        }
        // Safari / Mac-style connector OAuth: relay redirects to arbos://connectors/callback.
        .onOpenURL { url in
            // Invite links (D88): arbos://join/<code> and arbos://join-group/<code> — what
            // arbos.life/s/<code> and /g/<code> hand to the app. Anything can send the app one
            // of these (a web page, another app, a link in a message), so the join waits for
            // the Join button in `InviteLink.confirm` — joining puts the person in a room with
            // whoever minted the code.
            if let invite = InviteLink(url: url) {
                pendingInvite = invite
                return
            }
            guard AuthEndpoints.isConnectorCallback(url) else { return }
            model.refreshConnectors()
        }
        .confirm(item: $pendingInvite, spec: InviteLink.confirm) { invite in
            Task {
                switch invite.kind {
                case .server: await model.joinFromLink(code: invite.code)
                case .group: await model.joinGroupFromLink(code: invite.code)
                }
            }
        }
    }

    private var expectedPeople: [CallPerson] { calls.call.map(model.expectedCallPeople) ?? [] }

    /// Shake / screenshot → capture the current screen and open the feedback sheet.
    private func openFeedbackFromGesture() {
        guard model.isPaired else { return }
        feedback.open(screen: model.path.isEmpty ? "home" : "chat",
                      chatId: model.path.last.map { if case .chat(let c, _) = $0 { return c } else { return "" } },
                      projectId: model.path.last.map { if case .chat(_, let p) = $0 { return p } else { return "" } })
    }

    /// Ask once, a beat after the home screen appears (never in the fixture or a live demo).
    private func schedulePrimer(_ paired: Bool) {
        guard paired, !model.isFixture, !DebugHooks.liveDemo, DebugHooks.show == nil, push.shouldShowPrimer, !showPrimer,
              model.firstLogin == nil else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) {
            // Never on top of another sheet (feedback, settings, profile setup…) or the first-login screen.
            if push.shouldShowPrimer, model.isPaired, !feedback.isPresented, !model.needsProfileSetup, model.firstLogin == nil { showPrimer = true }
        }
    }

    private var preferredScheme: ColorScheme? {
        switch model.appearance {
        case .system: return nil
        case .dark: return .dark
        case .light: return .light
        }
    }
}

/// Root chrome after sign-in: the home list alone. The Telegram-style tab bar
/// (Contacts · Calls · Chats · Settings, 2026-10-23) is gone (Jacob, 2026-10-25,
/// [feedback iphone-bottom-search-bar-2026-10-25]): Home's bottom edge is one
/// app-wide search bar (`HomeChrome.bottom`). Settings opens from the account
/// circle; people are found through + (New message) and the search bar.
struct MainShellView: View {
    var body: some View {
        HomeView()
    }
}

/// The profile sheet: a NavigationStack rooted on the page so a tapped member
/// (or an agent's owner) is pushed and its back chevron (the page's own
/// `ProfileTopBar`) pops to the page as it was.
private struct ProfileSheet: View {
    @EnvironmentObject private var model: AppModel
    let target: ProfileTarget

    var body: some View {
        if target.pushesPeople {
            NavigationStack(path: $model.profilePath) {
                page(target)
                    .toolbar(.hidden, for: .navigationBar)
                    .navigationDestination(for: ProfileTarget.self) { pushed in
                        page(pushed).toolbar(.hidden, for: .navigationBar)
                    }
            }
        } else {
            page(target)
        }
    }

    @ViewBuilder
    private func page(_ target: ProfileTarget) -> some View {
        switch target {
        case .user(let p): UserProfileView(person: p)
        case .group(let chatId, let projectId): GroupProfileView(chatId: chatId, projectId: projectId)
        case .agent(let pid): AgentProfileView(projectId: pid, fallback: model.foreignAgent(pid))
        case .server(let sid): ServerProfileView(serverId: sid)
        case .channel(let cid): ChannelProfileView(channelId: cid)
        }
    }
}
