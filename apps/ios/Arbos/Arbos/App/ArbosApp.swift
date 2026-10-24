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
        }
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
            #endif
            if DebugHooks.show == "onboarding" {
                model.demoFirstLogin(stage: DebugHooks.onboardingStage)
                if DebugHooks.onboardingProfile { model.needsProfileSetup = true }
            }
        }
        .environment(\.theme, theme)
        .preferredColorScheme(preferredScheme)
        .tint(theme.text)
        .alert(item: $model.notice) { n in
            Alert(title: Text("Arbos"), message: Text(n.text), dismissButton: .default(Text("OK")))
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
            // Invite links (D88): arbos://join/<code> — what arbos.life/s/<code> hands to the app.
            if url.scheme == "arbos", url.host == "join" {
                let code = url.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
                Task { await model.joinFromLink(code: code) }
                return
            }
            // Group invite links: arbos://join-group/<code> — what arbos.life/g/<code> hands to the app.
            if url.scheme == "arbos", url.host == "join-group" {
                let code = url.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
                Task { await model.joinGroupFromLink(code: code) }
                return
            }
            guard AuthEndpoints.isConnectorCallback(url) else { return }
            model.refreshConnectors()
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

/// Telegram-style bottom tabs on the paired shell: Contacts, Calls, Chats, Settings.
enum MainTab: Int, Hashable, CaseIterable {
    case contacts, calls, chats, settings
}

/// Root chrome after sign-in: four pinned tabs (Telegram layout). Chats is the
/// existing home list; Contacts opens the directory; Calls is recent-call ready
/// (empty until history lands); Settings is the same Settings page as the sheet.
struct MainShellView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme

    var body: some View {
        TabView(selection: $model.mainTab) {
            ContactsTabView()
                .tabItem { Label("Contacts", systemImage: "person.crop.circle.fill") }
                .tag(MainTab.contacts)
                .accessibilityIdentifier("tab-contacts")
            CallsTabView()
                .tabItem { Label("Calls", systemImage: "phone.fill") }
                .tag(MainTab.calls)
                .accessibilityIdentifier("tab-calls")
            HomeView()
                .tabItem { Label("Chats", systemImage: "bubble.left.and.bubble.right.fill") }
                .tag(MainTab.chats)
                .badge(model.unreadTotal)
                .accessibilityIdentifier("tab-chats")
            SettingsView()
                .tabItem { Label("Settings", systemImage: "gearshape.fill") }
                .tag(MainTab.settings)
                .accessibilityIdentifier("tab-settings")
        }
        .tint(theme.accent)
        .toolbarBackground(theme.bg, for: .tabBar)
        .toolbarBackground(.visible, for: .tabBar)
    }
}

/// Directory of Arbos people (Telegram Contacts): tap opens a DM on the Chats tab.
private struct ContactsTabView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @State private var query = ""
    @State private var opening = false
    @State private var loading = false

    private var trimmed: String {
        query.trimmingCharacters(in: .whitespaces)
            .trimmingCharacters(in: CharacterSet(charactersIn: "@"))
            .lowercased()
    }

    private var people: [Person] {
        let base = model.directory.filter { $0.id != model.user?.id }
        let filtered: [Person]
        if trimmed.isEmpty {
            filtered = base
        } else {
            filtered = base.filter {
                $0.handle.lowercased().contains(trimmed)
                    || ($0.name ?? "").lowercased().contains(trimmed)
            }
        }
        return filtered.sorted {
            $0.displayName.localizedCaseInsensitiveCompare($1.displayName) == .orderedAscending
        }
    }

    var body: some View {
        NavigationStack {
            ZStack(alignment: .top) {
                theme.bg.ignoresSafeArea()
                List {
                    Color.clear.frame(height: 108)
                        .listRowInsets(EdgeInsets())
                        .listRowSeparator(.hidden)
                        .listRowBackground(Color.clear)
                        .accessibilityHidden(true)
                    if people.isEmpty {
                        contactsEmpty
                            .listRowInsets(EdgeInsets())
                            .listRowSeparator(.hidden)
                            .listRowBackground(Color.clear)
                    } else {
                        ForEach(people, id: \.id) { person in
                            Button {
                                open(person)
                            } label: {
                                PersonRow(person: person)
                            }
                            .buttonStyle(.borderless)
                            .disabled(opening)
                            .listRowInsets(EdgeInsets(top: 0, leading: 8, bottom: 0, trailing: 8))
                            .listRowSeparator(.hidden)
                            .listRowBackground(theme.bg)
                        }
                    }
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                .background(theme.bg)
                contactsTopBar
            }
            .toolbar(.hidden, for: .navigationBar)
            .task {
                loading = model.directory.isEmpty
                await model.loadDirectory()
                loading = false
            }
        }
    }

    private var contactsTopBar: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Contacts")
                .font(theme.font(Theme.body, weight: .semibold))
                .foregroundStyle(theme.text)
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").foregroundStyle(theme.icon)
                TextField("", text: $query, prompt: Text("Search").foregroundStyle(theme.placeholder))
                    .font(theme.font(Theme.body))
                    .foregroundStyle(theme.text)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                if !query.isEmpty {
                    Button { query = "" } label: {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(theme.icon)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Clear search")
                }
            }
            .padding(.horizontal, 14)
            .frame(height: Theme.control)
            .background(Capsule().fill(theme.field))
            .overlay(Capsule().strokeBorder(theme.fieldBorder, lineWidth: 1))
        }
        .padding(.horizontal, 20)
        .padding(.top, 6)
        .padding(.bottom, 8)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .background(theme.bg)
        .zIndex(1)
    }

    @ViewBuilder
    private var contactsEmpty: some View {
        if loading {
            HStack { Spacer(); ProgressView().tint(theme.muted); Spacer() }
                .padding(.top, 80)
        } else {
            Text(trimmed.isEmpty
                 ? (model.credentials == nil
                    ? "Sign in to see who else is on Arbos."
                    : "No contacts yet. People on Arbos will show up here.")
                 : "No contacts match \"\(query.trimmingCharacters(in: .whitespaces))\".")
                .font(theme.font(Theme.small))
                .foregroundStyle(theme.muted)
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity)
                .padding(.horizontal, 36)
                .padding(.top, 80)
        }
    }

    private func open(_ person: Person) {
        guard !opening else { return }
        opening = true
        model.mainTab = .chats
        Task {
            _ = await model.openDM(handle: person.handle)
            opening = false
        }
    }
}

/// Recent calls tab (Telegram Calls). Empty until call history is stored on device.
private struct CallsTabView: View {
    @Environment(\.theme) private var theme

    var body: some View {
        NavigationStack {
            ZStack(alignment: .top) {
                theme.bg.ignoresSafeArea()
                VStack(spacing: 10) {
                    Spacer().frame(height: 80)
                    Image(systemName: "phone")
                        .font(.system(size: 36, weight: .light))
                        .foregroundStyle(theme.muted)
                    Text("No recent calls")
                        .font(theme.font(Theme.body, weight: .medium))
                        .foregroundStyle(theme.text)
                    Text("Voice and video calls with people on Arbos will show up here.")
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 40)
                    Spacer()
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                HStack {
                    Text("Calls")
                        .font(theme.font(Theme.body, weight: .semibold))
                        .foregroundStyle(theme.text)
                    Spacer()
                }
                .padding(.horizontal, 20)
                .padding(.top, 6)
                .frame(height: Theme.control + 12, alignment: .top)
                .zIndex(1)
            }
            .toolbar(.hidden, for: .navigationBar)
        }
    }
}

/// The profile sheet: one page, or — for pages whose member rows open people —
/// a NavigationStack rooted on it so a tapped member is pushed and its back
/// chevron (the page's own `ProfileTopBar`) pops to the page as it was. The
/// agent page brings its own NavigationStack and so stays a plain root.
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
