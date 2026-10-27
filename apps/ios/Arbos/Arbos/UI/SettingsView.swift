// Settings, Apple-Account style in Arbos colours (D80, Jacob 2026-10-17): a
// centred profile header (large avatar, bold name, grey handle), then a few
// short grouped cards of icon-tile rows that push to subpages. Everything that
// was on the old flat form is still here, regrouped:
//
//   Account              Credits · Connected apps · On this iPhone
//   Agents & Computer    Machine
//   Notifications        Notifications (system permission + relay push)
//   Appearance           Theme · Surface (Glass / Matte)
//   Help & About         Send feedback · About · Relay (debug builds)
//   Sign out
//
// Reference: media/design-parity/reference/apple/apple-account-settings-light.png.
// Page #FCFCFC / #141414, cards = the assistant-bubble grey, our circular back
// button, 17 pt labels, muted monochrome icon tiles, hairlines inset past the tile.

import SwiftUI
import UIKit
import UserNotifications

struct SettingsView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @Environment(\.scenePhase) private var scenePhase
    @ObservedObject private var push = PushManager.shared
    @State private var confirmUnpair = false
    /// Connector errors — the root `notice` alert sits under this sheet and looks like a silent fail.
    @State private var connectorError: UserError?
    /// The avatar picker (Generate | Upload, Reset) over my picture — the agents' sheet
    /// (Jacob, 2026-10-25); `ARBOS_SETTINGS_PAGE=avatar[-generate|-upload]` opens it (rig).
    @State private var showAvatarPicker: AvatarPickerTab?
    @State private var showProfileEdit = false
    /// Profile | Settings — the same pill as the person and group panes.
    /// Settings is the landing tab; a `landing` of `profile` lands on Profile.
    @State private var tab: String
    /// The page the sheet lands on: a search result's page ([feedback
    /// search-settings-ios-2026-10-26]) or the rig's `ARBOS_SETTINGS_PAGE`
    /// (appearance · textsize · connectors · machine | machine-expanded · credits · notifications ·
    /// privacy · about · devices · relay · secrets | vault | connect | new-secret | secret:<ENV>).
    private let landing: String?
    @State private var showAppearancePage: Bool
    @State private var showTextSizePage: Bool
    @State private var showConnectorsPage: Bool
    @State private var showMachinePage: Bool
    @State private var showCreditsPage: Bool
    @State private var showNotificationsPage: Bool
    @State private var showPrivacyPage: Bool
    @State private var showAboutPage: Bool
    @State private var showDevicesPage: Bool
    @State private var showRelayPage: Bool
    @State private var showSecretsPage: Bool

    /// The pages the Settings list shows (`settingsSections`), as search routes;
    /// the structure test checks the shared index finds each of them.
    static let listedPages: [GlobalSearch.SettingsPageRoute] = [
        .credits, .connectedApps, .secrets, .devices, .machine, .notifications, .privacy, .appearance, .textSize, .about,
    ]

    init(landing: String? = DebugHooks.settingsPage) {
        self.landing = landing
        _tab = State(initialValue: landing == "profile" ? "Profile" : "Settings")
        _showAppearancePage = State(initialValue: landing == "appearance")
        _showTextSizePage = State(initialValue: landing == "textsize")
        _showConnectorsPage = State(initialValue: landing == "connectors")
        _showMachinePage = State(initialValue: landing == "machine" || landing == "machine-expanded")
        _showCreditsPage = State(initialValue: landing == "credits")
        _showNotificationsPage = State(initialValue: landing == "notifications")
        _showPrivacyPage = State(initialValue: landing == "privacy")
        _showAboutPage = State(initialValue: landing == "about")
        _showDevicesPage = State(initialValue: landing == "devices")
        _showRelayPage = State(initialValue: landing == "relay")
        _showSecretsPage = State(initialValue: ["secrets", "vault", "connect", "new-secret"].contains(landing ?? "") || landing?.hasPrefix("secret:") == true)
    }

    var body: some View {
        NavigationStack {
            // The shared pane (D93): my picture (tap → the photo picker), name,
            // @handle; Profile | Settings; Sign out pinned under Settings — the
            // Mac me pane, row for row.
            ZStack(alignment: .top) {
                theme.bg.ignoresSafeArea()
                ScrollView {
                    VStack(spacing: 18) {
                        Color.clear.frame(height: PaneKit.pageTop)
                        ProfilePane(
                            hero: PaneHero(
                                picture: .image(AnyView(profilePicture)),
                                name: model.user?.name ?? model.user?.login ?? "Arbos",
                                subtitle: who
                            ),
                            tabs: PaneKinds.ownProfile,
                            selected: $tab,
                            body: tab == "Profile" ? profileSections : settingsSections,
                            footer: tab == "Profile" ? [] : settingsFooter
                        )
                        Color.clear.frame(height: 40)
                    }
                }
                .scrollIndicators(.visible)
                // Settings is a sheet (from Home's account circle): back closes it.
                ProfileTopBar(onBack: { dismiss() }, screen: .ownProfile) { EmptyView() }
            }
            .toolbar(.hidden, for: .navigationBar)
            .navigationBarBackButtonHidden(true)
            .confirmationDialog("Sign out?", isPresented: $confirmUnpair, titleVisibility: .visible) {
                Button("Sign out", role: .destructive) {
                    if model.tokens != nil { model.signOut() } else { model.unpair() }
                    dismiss()
                }
            } message: {
                Text(model.tokens != nil ? "This device is signed out of the relay; sign in with GitHub to come back." : "You will need a new pairing code to connect again.")
            }
            .onAppear {
                model.refreshConnectors()
                Task { await push.refreshStatus() }
                Task { await model.refreshSecrets() }
                Task { await model.refreshDevices() }
                Task { await model.refreshGoals() }
                if let page = landing, page.hasPrefix("avatar") {
                    tab = "Profile"
                    let initial: AvatarPickerTab = page.hasSuffix("upload") ? .upload : .generate
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { showAvatarPicker = initial }
                }
            }
            // After Google OAuth in Safari, refresh so "Needs sign-in" flips to Connected.
            .onChange(of: scenePhase) { _, phase in
                if phase == .active {
                    model.refreshConnectors()
                    Task { await push.refreshStatus() }
                }
            }
            // The sheet's own toast layer (Jacob 2026-10-26): a failure while Settings is up —
            // Connect, a picture save, a secret — is the shared note at the top of this sheet,
            // not an alert and not a toast hidden under it.
            .overlay(alignment: .top) {
                ToastHost()
                    .environmentObject(model)
                    .environment(\.theme, theme)
            }
            .sheet(item: $showAvatarPicker) { initial in
                AvatarPickerSheet(target: .me, initialTab: initial).environmentObject(model).environment(\.theme, theme)
            }
            .navigationDestination(isPresented: $showAppearancePage) { AppearancePage() }
            .navigationDestination(isPresented: $showTextSizePage) { TextSizePage() }
            .navigationDestination(isPresented: $showConnectorsPage) { ConnectorsPage(connectorError: $connectorError) }
            .navigationDestination(isPresented: $showMachinePage) { MachinePage(openAgent: openAgentFromSettings) }
            .navigationDestination(isPresented: $showCreditsPage) { CreditsPage() }
            .navigationDestination(isPresented: $showNotificationsPage) { NotificationsPage() }
            .navigationDestination(isPresented: $showPrivacyPage) { PrivacyPage() }
            .navigationDestination(isPresented: $showAboutPage) { AboutPage() }
            .navigationDestination(isPresented: $showDevicesPage) { DevicesPage() }
            .navigationDestination(isPresented: $showRelayPage) {
                // RelayPage only exists in Debug; the Release (TestFlight) build has no such type.
                #if DEBUG
                RelayPage()
                #else
                EmptyView()
                #endif
            }
            .navigationDestination(isPresented: $showSecretsPage) { SecretsPage(landing: landing) }
            .sheet(isPresented: $showProfileEdit) {
                NavigationStack {
                    ProfileEditView()
                }
                .environmentObject(model)
                .environment(\.theme, theme)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
                .presentationBackground(theme.bg)
                .preferredColorScheme(theme.isDark ? .dark : .light)
            }
        }
        .tint(theme.text)
        .preferredColorScheme(theme.isDark ? .dark : .light)
        .presentationBackground(theme.bg)
    }

    /// "@login", or where we are pointed when not signed in.
    private var who: String {
        model.user.map { "@\($0.login)" } ?? (model.relayUrl.isEmpty ? "Not signed in" : model.relayUrl)
    }

    /// Settings: the grouped cards (the Mac me pane's Settings tab).
    private var settingsSections: [PaneSection] {
        let inset = SettingsRow.separatorInset
        var out: [PaneSection] = []
        var account: [AnyView] = [
            AnyView(NavigationLink { CreditsPage() } label: {
                SettingsRow(icon: "creditcard.fill", title: "Credits", value: creditsSummary)
            }),
        ]
        if model.credentials != nil {
            account.append(AnyView(NavigationLink { ConnectorsPage(connectorError: $connectorError) } label: {
                SettingsRow(icon: "link", title: "Connected apps", value: connectorsSummary)
            }))
            // D83: a secret is "an app you connected by key"; it sits right after.
            account.append(AnyView(NavigationLink { SecretsPage() } label: {
                SettingsRow(icon: "key.fill", title: "Secrets", value: model.secretsRowValue,
                            valueTint: model.secrets?.paused == true ? theme.danger : nil)
            }))
        }
        if model.credentials != nil || model.isFixture {
            // Devices (Oct 23): every GitHub sign-in is one; sign another out here.
            account.append(AnyView(NavigationLink { DevicesPage() } label: {
                SettingsRow(icon: "laptopcomputer", title: "Devices", value: devicesSummary)
            }))
        }
        if !model.isFixture {
            account.append(AnyView(SettingsRow(icon: "iphone", title: "On this iPhone", value: storageLine, chevron: false)))
        }
        out.append(.card(account, separatorInset: inset))
        out.append(.card([
            AnyView(NavigationLink { MachinePage(openAgent: openAgentFromSettings) } label: {
                SettingsRow(icon: "desktopcomputer", title: "Machine", value: machineSummary, dot: machineDotColor(model.machine.state, theme: theme))
            }),
        ], separatorInset: inset))
        out.append(.footnote(model.machine.alwaysOn
                             ? "Your one cloud computer, always on. Agents are workspaces on it."
                             : "Your one cloud computer. A message to any agent wakes it."))
        out.append(.card([
            AnyView(NavigationLink { NotificationsPage() } label: {
                SettingsRow(icon: "bell.badge.fill", title: "Notifications", value: notificationsSummary)
            }),
            AnyView(NavigationLink { PrivacyPage() } label: {
                SettingsRow(icon: "hand.raised.fill", title: "Privacy", value: privacySummary)
            }),
            AnyView(NavigationLink { AppearancePage() } label: {
                SettingsRow(icon: "circle.lefthalf.filled", title: "Appearance", value: appearanceSummary)
            }),
            AnyView(NavigationLink { TextSizePage() } label: {
                SettingsRow(icon: "textformat.size", title: "Text Size", value: TextScale.label(model.textScale))
            }),
        ], separatorInset: inset))
        var about: [AnyView] = [
            AnyView(Button {
                dismiss()
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.45) { FeedbackCenter.shared.open(screen: "settings") }
            } label: {
                SettingsRow(icon: "exclamationmark.bubble.fill", title: "Send feedback")
            }
            .buttonStyle(.plain)),
            AnyView(NavigationLink { AboutPage() } label: {
                SettingsRow(icon: "info.circle.fill", title: "About", value: SettingsView.versionLine)
            }),
        ]
        #if DEBUG
        about.append(AnyView(NavigationLink { RelayPage() } label: {
            SettingsRow(icon: "network", title: "Relay", value: URL(string: model.relayUrl)?.host ?? model.relayUrl)
        }))
        #endif
        out.append(.card(about, separatorInset: inset))
        return out
    }

    /// Pinned under Settings: Sign out.
    /// Settings › Machine › "Ask X to free space": close the sheet, then land in that
    /// agent's chat (the Send-feedback row's timing).
    private func openAgentFromSettings(_ project: Project) {
        dismiss()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.45) { model.open(project: project) }
    }

    private var settingsFooter: [PaneSection] {
        [.card([AnyView(PaneDangerRow(title: model.tokens != nil ? "Sign out" : "Sign out this device") { confirmUnpair = true })])]
    }

    /// Profile: who I am on the relay — Name and Handle (tap → the edit sheet),
    /// Remove picture when one was uploaded; the Mac me pane's Profile tab.
    private var profileSections: [PaneSection] {
        let canEdit = model.user != nil && !model.isFixture
        let rows: [AnyView] = [
            AnyView(Button { editProfile() } label: {
                PaneRow(title: "Name", value: model.user?.name ?? model.user?.login ?? "Arbos", valueTint: theme.text)
            }
            .buttonStyle(.plain)
            .disabled(!canEdit)),
            AnyView(Button { editProfile() } label: {
                PaneRow(title: "Handle", value: who, valueTint: theme.text)
            }
            .buttonStyle(.plain)
            .disabled(!canEdit)),
        ]
        return [
            .card(rows),
            .footnote("Your name, handle and picture are what other people and their agents see, here and on the Mac. Signed in with GitHub."),
        ]
    }

    private func editProfile() {
        guard model.user != nil, !model.isFixture else { return }
        showProfileEdit = true
    }

    // MARK: Profile picture

    /// The hero picture with the crayon: tap → the avatar picker (Generate | Upload, Reset),
    /// the same sheet an agent's picture opens.
    private var profilePicture: some View {
        Button { showAvatarPicker = .generate } label: {
            PersonAvatar(url: model.user?.avatarUrl, handle: model.user?.login ?? "?", size: PaneKit.heroAvatar,
                         name: model.user?.name, id: model.user?.id,
                         colorIndex: model.user?.avatarColorIndexEffective)
                .paneCrayon(model.user != nil)
        }
        .buttonStyle(.plain)
        .disabled(model.user == nil)
        .accessibilityLabel("Change profile picture")
    }

    // MARK: Row summaries

    private var devicesSummary: String? {
        switch model.devices.count {
        case 0: return nil
        case 1: return "1 device"
        case let n: return "\(n) devices"
        }
    }

    private var creditsSummary: String? {
        guard let c = model.credits else { return nil }
        return String(format: "$%.0f left", c.balanceUsd)
    }

    private var connectorsSummary: String? {
        let n = model.connectors.items.filter { $0.status == .connected }.count
        return n == 0 ? nil : "\(n) connected"
    }

    private var machineSummary: String {
        if let n = model.waitlistPosition { return "In line · number \(n)" }
        if model.machine.alwaysOn, !model.machine.held, model.machineBusy { return "Restarting…" }
        return model.machine.label
    }

    private var notificationsSummary: String {
        switch push.status {
        case .authorized, .provisional, .ephemeral: return "On"
        case .denied: return "Off"
        case .notDetermined: return "Not asked"
        @unknown default: return ""
        }
    }

    private var appearanceSummary: String {
        return "\(model.appearance.label) · \(model.surface.label)"
    }

    private var privacySummary: String {
        (GroupAddPolicy(rawValue: model.groupAddPolicy) ?? .anyone).summary
    }

    /// "12.4 MB · 23 chats" from the on-device store (#929).
    private var storageLine: String {
        let u = model.storeUsage
        let size = ByteCountFormatter.string(fromByteCount: u.bytes, countStyle: .file)
        return u.docs == 0 ? "Nothing saved yet" : "\(size) · \(u.docs) chat\(u.docs == 1 ? "" : "s")"
    }

    static func relative(_ ms: Int64) -> String {
        let date = Date(timeIntervalSince1970: Double(ms) / 1000)
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .short
        return f.localizedString(for: date, relativeTo: Date())
    }

    static var versionLine: String {
        let info = Bundle.main.infoDictionary
        let v = info?["CFBundleShortVersionString"] as? String ?? "?"
        let b = info?["CFBundleVersion"] as? String ?? "?"
        return "\(v) (\(b))"
    }
}


// MARK: - Building blocks (Apple structure, Arbos skin)

/// A settings screen: our circular back button, centred title, scrolling content on the page colour.
struct SettingsPage<Content: View>: View {
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let title: String
    var onBack: (() -> Void)? = nil
    @ViewBuilder var content: () -> Content

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(spacing: 24) {
                        // Clear the custom header + the soft end of `headerBlur`
                        // so the first card (Credits Balance, etc.) is not frosted.
                        Color.clear.frame(height: 82)
                        content()
                        Color.clear.frame(height: 24)
                    }
                    .padding(.horizontal, PaneKit.inset)
                }
                .onAppear {
                    // Rig hook: land on one card of a long page (`ARBOS_SETTINGS_SCROLL=people`).
                    guard let anchor = DebugHooks.settingsScroll else { return }
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { proxy.scrollTo(anchor, anchor: .top) }
                }
            }
            .scrollIndicators(.visible)
            // Apple Account pull-down: frosted fade under the title so the
            // avatar softens as it scrolls / rubber-bands into the header.
            headerBlur
            HStack {
                Button { (onBack ?? { dismiss() })() } label: {
                    GlassCircle {
                        Image(systemName: "chevron.left")
                            .font(.system(size: 17, weight: .semibold))
                            .foregroundStyle(theme.text)
                    }
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Back")
                Spacer()
            }
            .padding(.horizontal, 20)
            .padding(.top, 16)
            .overlay {
                // Navigation chrome stays at the system size (Apple's Text Size does the same).
                Text(title)
                    .font(.system(size: Theme.body, weight: .semibold))
                    .foregroundStyle(theme.text)
                    .padding(.top, 16)
            }
        }
        .toolbar(.hidden, for: .navigationBar)
        .navigationBarBackButtonHidden(true)
        // Custom back hides the system edge-swipe; ScrollView in this sheet also
        // eats InteractivePopGestureEnabler. Same swipe-right → back as ChatView.
        .simultaneousGesture(settingsBackSwipe)
    }

    /// Swipe right pops a pushed settings page (Credits, Machine, …). Root
    /// Settings passes `onBack` to dismiss the sheet — leave that to the
    /// back button / sheet drag so a casual swipe does not close Settings.
    private var settingsBackSwipe: some Gesture {
        DragGesture(minimumDistance: 50, coordinateSpace: .local)
            .onEnded { value in
                guard onBack == nil else { return }
                let dx = value.translation.width
                let absX = abs(dx), absY = abs(value.translation.height)
                let vx = value.velocity.width, vy = value.velocity.height
                if dx > 0, absX > 70, absX > absY * 1.25, abs(vx) >= abs(vy) || absX > 120 {
                    dismiss()
                }
            }
    }

    /// Strip under the back button + title — Apple Account always frosts what
    /// peeks under the nav on pull-down, so Settings keeps the material even
    /// when the rest of the app is on Matte.
    private var headerBlur: some View {
        ZStack {
            Rectangle().fill(.ultraThinMaterial)
            theme.bg.opacity(0.78)
        }
        .frame(height: 110)
        .mask(LinearGradient(colors: [.black, .black, .clear], startPoint: .top, endPoint: .bottom))
        .ignoresSafeArea(edges: .top)
        .allowsHitTesting(false)
    }
}

/// One rounded inset card holding rows: the pane kit's card, hairlines inset past the icon tiles.
struct SettingsGroup<Rows: View>: View {
    var header: String? = nil
    var footer: String? = nil
    @ViewBuilder var rows: () -> Rows

    var body: some View {
        PaneCard(header: header, footer: footer, separatorInset: SettingsRow.separatorInset, rows: rows)
    }
}

/// Icon tile · title · optional trailing value · chevron: the pane kit's row.
struct SettingsRow: View {
    let icon: String
    let title: String
    var value: String? = nil
    var chevron: Bool = true
    /// Small status dot drawn on the tile (machine state).
    var dot: Color? = nil
    /// Colour for the trailing value (red `Paused`).
    var valueTint: Color? = nil

    static let height: CGFloat = PaneKit.rowHeight
    static let tile: CGFloat = PaneKit.tile
    static let separatorInset: CGFloat = PaneKit.separatorInset(.tile(""))

    var body: some View {
        PaneRow(leading: .tile(icon, dot: dot), title: title, value: value, valueTint: valueTint, chevron: chevron)
    }
}

/// Label · trailing value, for information rows inside a subpage card.
struct SettingsValueRow<Trailing: View>: View {
    @Environment(\.theme) private var theme
    let title: String
    @ViewBuilder var trailing: () -> Trailing

    init(_ title: String, value: String) where Trailing == Text {
        self.title = title
        self.trailing = { Text(value) }
    }

    init(_ title: String, @ViewBuilder trailing: @escaping () -> Trailing) {
        self.title = title
        self.trailing = trailing
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(title).font(theme.font(Theme.body)).foregroundStyle(theme.text)
            Spacer(minLength: 8)
            trailing()
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.muted)
                .multilineTextAlignment(.trailing)
        }
        .padding(.horizontal, 16)
        .frame(minHeight: SettingsRow.height)
    }
}

/// A row that picks one option (checkmark on the chosen one), Apple's list-picker look.
struct SettingsChoiceRow: View {
    @Environment(\.theme) private var theme
    let title: String
    let selected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack {
                Text(title).font(theme.font(Theme.body)).foregroundStyle(theme.text)
                Spacer()
                if selected {
                    Image(systemName: "checkmark").font(.system(size: 15, weight: .semibold)).foregroundStyle(theme.text)
                }
            }
            .padding(.horizontal, 16)
            .frame(minHeight: SettingsRow.height)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// A text action row (Start machine, Connect, Switch relay).
struct SettingsActionRow: View {
    @Environment(\.theme) private var theme
    let title: String
    var destructive: Bool = false
    var disabled: Bool = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            // Apple's Settings tints button rows (blue, red when destructive) so
            // they read as actions, not values.
            Text(title)
                .font(theme.font(Theme.body))
                .foregroundStyle(destructive ? theme.danger : theme.accent)
                .frame(maxWidth: .infinity, minHeight: SettingsRow.height, alignment: .leading)
                .padding(.horizontal, 16)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.5 : 1)
    }
}

// MARK: - Subpages

struct CreditsPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @State private var showTopUpFeedback = false

    var body: some View {
        SettingsPage(title: "Credits") {
            if let c = model.credits {
                SettingsGroup(footer: "Credits pay for your agents' model use. Machine time is billed separately. During the beta, usage credit is included — request a top-up when you run low.") {
                    SettingsValueRow("Balance", value: String(format: "$%.2f left of $%.0f", c.balanceUsd, c.grantedUsd))
                    if let cap = c.dailyCapUsd {
                        SettingsValueRow("Today", value: String(format: "$%.2f of $%.0f cap", c.spentTodayUsd, cap))
                    } else {
                        SettingsValueRow("Today", value: String(format: "$%.2f", c.spentTodayUsd))
                    }
                }
            } else {
                SettingsGroup { SettingsValueRow("Balance", value: "Not available yet") }
            }
            SettingsGroup {
                SettingsActionRow(title: "Request a top-up") {
                    FeedbackCenter.shared.context = FeedbackCenter.Context(screen: "credits")
                    FeedbackCenter.shared.screenshot = FeedbackCenter.captureScreen()
                    showTopUpFeedback = true
                }
            }
        }
        .sheet(isPresented: $showTopUpFeedback) {
            FeedbackSheet()
                .environmentObject(model)
                .environment(\.theme, theme)
        }
    }
}

/// Settings › Machine ([feedback machine-page-ios-2026-10-26]): the plan is pure
/// (`MachineMetrics.plan`, the Mac's `machine::plan`); every row here is one of its entries. The
/// box report is polled every `MachineMetrics.pollSeconds` through `.task`, which SwiftUI cancels
/// the moment the page leaves the screen — the poll's whole lifecycle (`MachineMetrics.pollSeconds`).
struct MachinePage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    /// Settings passes this to close the sheet and land in the agent's chat ("Ask X to free space").
    var openAgent: ((Project) -> Void)? = nil
    /// Rig: `ARBOS_SETTINGS_PAGE=machine-expanded` lands with the breakdown unfolded.
    @State private var breakdownOpen = DebugHooks.settingsPage == "machine-expanded"
    @State private var showBiggerMachineFeedback = false

    private var report: BoxReport? { model.boxReport }
    private var live: BoxReport? { report.flatMap { $0.reachable ? $0 : nil } }

    var body: some View {
        let m = model.machine
        let plan = MachineMetrics.plan(machine: m, waitlisted: model.waitlistPosition != nil, report: report, breakdownOpen: breakdownOpen)
        SettingsPage(title: "Machine") {
            if let r = live, let w = MachineMetrics.warning(r), !plan.warning.isEmpty {
                SettingsGroup {
                    ForEach(Array(plan.warning.enumerated()), id: \.offset) { _, row in
                        switch row {
                        case .warning:
                            MachineWarningRow(warning: w)
                        case .askToFreeSpace:
                            if let ws = MachineMetrics.biggestDiskUser(r), let project = model.project(ws.projectId) {
                                SettingsActionRow(title: "Ask \(ws.name) to free space") { askToFreeSpace(project) }
                            }
                        case .requestBiggerMachineInWarning:
                            SettingsActionRow(title: "Request a bigger machine") { requestBiggerMachine() }
                        default:
                            EmptyView()
                        }
                    }
                }
            }
            SettingsGroup {
                ForEach(Array(plan.machine.enumerated()), id: \.offset) { _, row in
                    switch row {
                    case .whereItRuns:
                        SettingsValueRow("Where it runs", value: m.whereItRuns)
                    case .inLine:
                        SettingsValueRow("In line", value: "Number \(model.waitlistPosition ?? 0) · we'll let you know")
                    case .state:
                        SettingsValueRow("State") {
                            HStack(spacing: 7) {
                                if m.alwaysOn, !m.held, model.machineBusy {
                                    ProgressView().controlSize(.mini).tint(theme.muted)
                                    Text("Restarting…")
                                } else {
                                    Circle().fill(machineDotColor(m.state, theme: theme)).frame(width: 8, height: 8)
                                    Text(stateText(m))
                                }
                            }
                        }
                    case .spec:
                        SettingsValueRow("Spec", value: m.spec.isEmpty ? "Not reported yet" : m.spec)
                    case .machineTime:
                        if m.alwaysOn {
                            SettingsValueRow("Machine time today") {
                                VStack(alignment: .trailing, spacing: 2) {
                                    Text(String(format: "$%.2f", m.machineTodayUsd))
                                    Text("Not taken from credits").font(theme.font(Theme.tiny))
                                }
                            }
                        } else {
                            SettingsValueRow("Spend today", value: String(format: "$%.2f of $%.0f", m.spendTodayUsd, m.spendCapUsd))
                        }
                    case .workspaces:
                        SettingsValueRow("Workspaces", value: "\(m.workspaces.values.filter { $0 == .active }.count) running · \(m.workspaces.count) total")
                    case .sandbox:
                        SettingsValueRow("Sandbox") {
                            Text(m.sandboxId ?? "").font(theme.font(12, design: .monospaced)).lineLimit(1).truncationMode(.middle)
                        }
                    default:
                        EmptyView()
                    }
                }
            }
            if !plan.resources.isEmpty {
                SettingsGroup(header: "Resources") {
                    ForEach(Array(plan.resources.enumerated()), id: \.offset) { _, row in
                        switch row {
                        case .disk:
                            if let r = live, let g = MachineMetrics.diskGauge(r) { MachineGaugeRow(title: "Disk", gauge: g) }
                        case .memory:
                            if let r = live, let g = MachineMetrics.memGauge(r) { MachineGaugeRow(title: "Memory", gauge: g) }
                        case .cpu:
                            if let r = live, let g = MachineMetrics.cpuGauge(r) { MachineGaugeRow(title: "CPU", gauge: g) }
                        case .network:
                            if let r = live, let net = MachineMetrics.netValue(r) {
                                SettingsValueRow("Network") {
                                    VStack(alignment: .trailing, spacing: 2) {
                                        Text(net.0)
                                        if let today = net.1 { Text(today).font(theme.font(Theme.tiny)) }
                                    }
                                }
                            }
                        case .notAnswering:
                            PaneRow(title: "The machine did not answer",
                                    subtitle: report?.unreachableReason ?? "Trying again in a few seconds",
                                    titleTint: theme.muted)
                        default:
                            EmptyView()
                        }
                    }
                }
            }
            if let r = live, !plan.agents.isEmpty {
                // `ARBOS_SETTINGS_SCROLL=agents` lands here (rig).
                SettingsGroup(header: "Agents") {
                    ForEach(Array(plan.agents.enumerated()), id: \.offset) { _, row in
                        switch row {
                        case .runningNow:
                            SettingsValueRow("Running now", value: MachineMetrics.runningNow(r))
                        case .jobs:
                            SettingsValueRow("Background jobs", value: MachineMetrics.jobsValue(r.jobsRunning ?? 0))
                        case .breakdown:
                            Button {
                                withAnimation(.easeInOut(duration: 0.2)) { breakdownOpen.toggle() }
                            } label: {
                                HStack(spacing: 12) {
                                    Text("Per-agent usage").font(theme.font(Theme.body)).foregroundStyle(theme.text)
                                    Spacer(minLength: 8)
                                    Image(systemName: breakdownOpen ? "chevron.up" : "chevron.right")
                                        .font(.system(size: 14, weight: .semibold))
                                        .foregroundStyle(theme.placeholder)
                                }
                                .padding(.horizontal, 16)
                                .frame(minHeight: SettingsRow.height)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("machine-breakdown")
                        case .agent(let id):
                            if let ws = r.workspaces.first(where: { $0.projectId == id }) {
                                MachineAgentRow(workspace: ws, project: model.project(id))
                            }
                        default:
                            EmptyView()
                        }
                    }
                }
                .id("agents")
            }
            SettingsGroup(footer: m.alwaysOn
                          ? "Your one cloud computer, always on. Agents are workspaces on it. Machine time is billed separately from your credits."
                          : "Your one cloud computer. Agents are workspaces on it; a message to any agent wakes it.") {
                ForEach(Array(plan.actions.enumerated()), id: \.offset) { _, row in
                    switch row {
                    case .requestBiggerMachine:
                        SettingsActionRow(title: "Request a bigger machine") { requestBiggerMachine() }
                    case .restart:
                        SettingsActionRow(title: "Restart machine", disabled: model.machineBusy) { model.restartMachine() }
                    case .start:
                        SettingsActionRow(title: "Start machine", disabled: model.machineBusy) { model.startMachine() }
                    case .stop:
                        SettingsActionRow(title: "Stop machine", destructive: true, disabled: model.machineBusy) { model.stopMachine() }
                    default:
                        EmptyView()
                    }
                }
            }
        }
        .task {
            // The poll: fetch now, then every `pollSeconds` until the page leaves the screen
            // (SwiftUI cancels this task on disappear).
            while !Task.isCancelled {
                await model.refreshBoxReport()
                await model.refreshMachine()
                try? await Task.sleep(nanoseconds: UInt64(MachineMetrics.pollSeconds * 1_000_000_000))
            }
        }
        .sheet(isPresented: $showBiggerMachineFeedback) {
            FeedbackSheet()
                .environmentObject(model)
                .environment(\.theme, theme)
        }
    }

    /// "Awake · 42 m": the uptime folded into the state (Jacob 2026-10-26), as a duration
    /// (the Computer tab's), not "1 hr. ago".
    private func stateText(_ m: Machine) -> String {
        if m.state == .awake, let since = m.since, since > 0 {
            return "\(m.label) · \(AgentPane.durationS(max(0, (nowMs() - since) / 1000)))"
        }
        return m.label
    }

    /// A request to Jacob through the feedback sheet, as the Credits page's top-up (a Daytona
    /// snapshot carries a fixed spec; the resize is manual).
    private func requestBiggerMachine() {
        FeedbackCenter.shared.context = FeedbackCenter.Context(screen: "machine")
        FeedbackCenter.shared.screenshot = FeedbackCenter.captureScreen()
        showBiggerMachineFeedback = true
    }

    /// That agent's chat with the ask drafted in the composer (agentd's disk gate answers
    /// "free space" itself, #1641).
    private func askToFreeSpace(_ project: Project) {
        if let chat = model.singleChat(of: project.id), model.composerDraft(for: chat.id).isEmpty {
            model.setComposerDraft(MachineMetrics.freeSpaceAsk, for: chat.id)
        }
        if let openAgent { openAgent(project) } else { model.open(project: project) }
    }
}

/// Title left, value right (coloured past the thresholds), a 3 pt bar under both, an optional
/// muted line under the bar — the Mac's `machine::gauge_row`.
struct MachineGaugeRow: View {
    @Environment(\.theme) private var theme
    let title: String
    let gauge: MachineMetrics.Gauge

    static let barHeight: CGFloat = 3

    private var color: Color {
        switch gauge.level {
        case .ok: return theme.placeholder
        case .warn: return theme.warn
        case .crit: return theme.danger
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(title).font(theme.font(Theme.body)).foregroundStyle(theme.text)
                Spacer(minLength: 8)
                Text(gauge.value).font(theme.font(Theme.body)).foregroundStyle(gauge.level == .ok ? theme.muted : color)
            }
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(theme.text.opacity(0.09))
                    Capsule().fill(color).frame(width: max(0, geo.size.width * CGFloat(gauge.pct) / 100))
                }
            }
            .frame(height: Self.barHeight)
            if let sub = gauge.sub {
                Text(sub).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(1)
                    .padding(.top, -3)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 11)
        .frame(minHeight: SettingsRow.height)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(title) \(gauge.value)")
    }
}

/// The warning row at the top of the page: amber glyph, bold title, one plain sentence.
struct MachineWarningRow: View {
    @Environment(\.theme) private var theme
    let warning: MachineMetrics.Warning

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 22, weight: .regular))
                .foregroundStyle(theme.warn)
                .padding(.top, 1)
            VStack(alignment: .leading, spacing: 3) {
                Text(warning.title).font(theme.font(Theme.body, weight: .semibold)).foregroundStyle(theme.text)
                Text(warning.body).font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }
}

/// One workspace in the breakdown: picture, name over its status, CPU over memory · disk.
struct MachineAgentRow: View {
    @Environment(\.theme) private var theme
    let workspace: BoxWorkspace
    let project: Project?

    var body: some View {
        HStack(spacing: PaneKit.gap) {
            if let project {
                AgentAvatar(project: project, theme: theme, size: PaneKit.avatar, still: true)
            } else {
                NeutralAvatar(name: workspace.name, size: PaneKit.avatar)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(workspace.name).font(theme.font(Theme.body, weight: .medium)).foregroundStyle(theme.text).lineLimit(1)
                Text(MachineMetrics.agentStatus(workspace)).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(1)
            }
            Spacer(minLength: 8)
            // The numbers never wrap; the name column gives way (truncates) instead.
            VStack(alignment: .trailing, spacing: 2) {
                if let cpu = MachineMetrics.agentCpu(workspace) {
                    Text(cpu).font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                }
                if let md = MachineMetrics.agentMemDisk(workspace) {
                    Text(md).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                }
            }
            .lineLimit(1)
            .fixedSize(horizontal: true, vertical: false)
        }
        .padding(.horizontal, PaneKit.inset)
        .padding(.vertical, 8)
        .frame(minHeight: SettingsRow.height)
    }
}

struct ConnectorsPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Binding var connectorError: UserError?
    @State private var connectorErrorDetailsOpen = false
    /// Provider whose token paste field is open (`nil` = none).
    @State private var tokenFor: String?
    @State private var tokenDraft = ""
    /// Connected pill tapped: (connector id, label) awaiting the Disconnect confirmation.
    @State private var disconnecting: (String, String)?

    var body: some View {
        SettingsPage(title: "Connected apps") {
            // The page's note: under the page header, before the first card.
            if let connectorError {
                ErrorNote(id: "connector-error", error: connectorError, detailsOpen: $connectorErrorDetailsOpen)
            }
            SettingsGroup(footer: "Connect Gmail, Calendar, Slack, GitHub, or Harness Comms so your agents can use them. Tokens stay on the relay; the agent asks before it sends.") {
                ForEach(catalogProviders) { p in
                    connectorRow(p)
                }
            }
        }
        .onAppear { model.refreshConnectors() }
    }

    /// Full relay catalog (Google, Slack, GitHub, Harness Comms), or stand-ins before load.
    private var catalogProviders: [ConnectorProvider] {
        let fromRelay = model.connectors.providers
        if !fromRelay.isEmpty { return fromRelay }
        if model.connectors.loaded { return [] }
        // oauthConfigured: false until the catalog says otherwise — Connect
        // before load used to 409 with ARBOS_GOOGLE_CLIENT_* in the alert.
        return [
            ConnectorProvider(provider: "google", label: "Google (Gmail, Calendar)", connect: "oauth", oauthConfigured: false),
            ConnectorProvider(provider: "slack", label: "Slack", connect: "oauth_or_token", oauthConfigured: false),
            ConnectorProvider(provider: "github", label: "GitHub", connect: "oauth_or_token", oauthConfigured: false),
            ConnectorProvider(provider: "harness_comms", label: "Harness Comms (Beeper + Gmail)", connect: "token", oauthConfigured: true),
        ]
    }

    /// Grok's Plugins row (web-03): icon tile, title over a one-line status, one pill
    /// on the right — "Connect" (grey) or "✓ Connected" (green tint). Providers that
    /// take both OAuth and a token get a menu on the pill; token-only ones open the
    /// paste field directly; a connected pill asks before disconnecting.
    @ViewBuilder
    private func connectorRow(_ p: ConnectorProvider) -> some View {
        let cn = model.connectors.connector(for: p.provider)
        let connected = cn?.status == .connected
        let tokenOpen = tokenFor == p.provider
        let busy = model.connectorsBusy || model.isFixture
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .center, spacing: 14) {
                RoundedRectangle(cornerRadius: 7.5, style: .continuous)
                    .fill(theme.isDark ? Color(hex: "#5C5C60")! : Color(hex: "#6E6E73")!)
                    .frame(width: SettingsRow.tile, height: SettingsRow.tile)
                    .overlay(
                        Image(systemName: connectorSymbol(p.provider))
                            .font(theme.font(15, weight: .semibold))
                            .foregroundStyle(.white)
                    )
                VStack(alignment: .leading, spacing: 2) {
                    // Long labels ("Harness Comms (Beeper + Gmail)") may run to four lines at the larger text sizes (the SE needs the fourth).
                    Text(p.label).font(theme.font(Theme.body)).foregroundStyle(theme.text).lineLimit(theme.textScale > 1.05 ? 4 : 2)
                    Text(connectorSubline(p, cn: cn))
                        .font(theme.font(Theme.tiny))
                        .foregroundStyle(theme.muted)
                        .lineLimit(2)
                }
                Spacer(minLength: 8)
                if connected, let id = cn?.id {
                    Button {
                        disconnecting = (id, p.label)
                    } label: {
                        connectorPill(text: "Connected", check: true, on: true)
                    }
                    .buttonStyle(.plain)
                    .disabled(model.connectorsBusy)
                } else if tokenOpen {
                    Button {
                        tokenFor = nil
                        tokenDraft = ""
                    } label: { connectorPill(text: "Cancel", check: false, on: false) }
                    .buttonStyle(.plain)
                } else if p.hasOauth && p.acceptsToken {
                    Menu {
                        Button("Sign in with \(shortName(p))") { startOauth(p) }
                        Button("Paste a token…") { openToken(p) }
                    } label: { connectorPill(text: "Connect", check: false, on: false) }
                    .disabled(busy)
                } else if p.hasOauth {
                    Button { startOauth(p) } label: { connectorPill(text: "Connect", check: false, on: false) }
                        .buttonStyle(.plain)
                        .disabled(busy)
                } else if p.acceptsToken {
                    Button { openToken(p) } label: { connectorPill(text: "Connect", check: false, on: false) }
                        .buttonStyle(.plain)
                        .disabled(busy)
                }
            }
            if tokenOpen {
                HStack(spacing: 8) {
                    SecureField("Paste a token, then Save", text: $tokenDraft)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.text)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                        .background(theme.field, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .overlay(
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .strokeBorder(theme.fieldBorder, lineWidth: 1) // control-size: exception inlineChips
                        )
                    Button("Save") { saveToken(p) }
                        .font(theme.font(Theme.small, weight: .semibold))
                        .buttonStyle(.borderless)
                        .foregroundStyle(theme.accent)
                        .disabled(busy || tokenDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                .padding(.leading, SettingsRow.tile + 14)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
        .frame(minHeight: 56)
        .confirmationDialog(
            "Disconnect \(disconnecting?.1 ?? "")?",
            isPresented: Binding(get: { disconnecting != nil && disconnecting?.1 == p.label },
                                 set: { if !$0 { disconnecting = nil } }),
            titleVisibility: .visible
        ) {
            Button("Disconnect", role: .destructive) {
                if let id = disconnecting?.0 { model.disconnectConnector(id: id) }
                disconnecting = nil
            }
            Button("Keep", role: .cancel) { disconnecting = nil }
        } message: {
            Text("Your agents stop using it until you connect it again.")
        }
    }

    /// The Grok "Add" / "✓ Added" pill: 30 pt tall, 13 pt semibold.
    private func connectorPill(text: String, check: Bool, on: Bool) -> some View {
        HStack(spacing: 5) {
            if check { Image(systemName: "checkmark").font(.system(size: 11, weight: .bold)) }
            Text(text).font(theme.font(13, weight: .semibold))
        }
        .foregroundStyle(on ? theme.online : theme.text)
        .padding(.horizontal, 12)
        .frame(height: 30)
        .background(Capsule().fill(on ? theme.online.opacity(0.14) : theme.field))
        .overlay(Capsule().strokeBorder(on ? theme.online.opacity(0.35) : theme.fieldBorder, lineWidth: 1))
        .contentShape(Capsule())
        .fixedSize()
    }

    private func connectorSymbol(_ provider: String) -> String {
        switch provider {
        case "google": return "envelope.fill"
        case "slack": return "number"
        case "github": return "chevron.left.forwardslash.chevron.right"
        case "harness_comms": return "antenna.radiowaves.left.and.right"
        default: return "puzzlepiece.extension.fill"
        }
    }

    private func shortName(_ p: ConnectorProvider) -> String {
        switch p.provider {
        case "google": return "Google"
        case "slack": return "Slack"
        case "github": return "GitHub"
        default: return p.label
        }
    }

    private func openToken(_ p: ConnectorProvider) {
        tokenFor = p.provider
        tokenDraft = ""
    }

    private func startOauth(_ p: ConnectorProvider) {
        tokenFor = nil
        tokenDraft = ""
        // Stand-ins claim oauth is ready; tapping Connect before the catalog
        // lands hits a 409 and surfaces env-var names. Wait, then explain.
        if !model.connectors.mock {
            if !model.connectors.loaded {
                model.refreshConnectors()
                connectorError = UserError.refused("connect \(p.label)", "Still loading connected apps. Try Connect again in a moment.")
                return
            }
            // Prefer the live catalog row (oauthConfigured) over a stale stand-in.
            let live = model.connectors.providers.first(where: { $0.provider == p.provider }) ?? p
            if !live.oauthConfigured {
                connectorError = UserError.refused("connect \(live.label)", "The OAuth client is not configured on the relay."
                    + (live.acceptsToken ? " Use Token… with an API key or personal access token instead." : ""))
                return
            }
            model.connectConnector(provider: live.provider)
            return
        }
        model.connectConnector(provider: p.provider)
    }

    private func saveToken(_ p: ConnectorProvider) {
        let token = tokenDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty else { return }
        tokenFor = nil
        tokenDraft = ""
        model.connectConnector(provider: p.provider, token: token)
    }

    private func connectorSubline(_ p: ConnectorProvider, cn: Connector?) -> String {
        if let cn, cn.status == .connected {
            return cn.label.isEmpty ? ConnectorStatus.connected.label : cn.label
        }
        // Prefer the real reason over a stuck "Needs sign-in" from a failed start.
        if model.connectors.loaded, !p.oauthConfigured, !model.connectors.mock, p.hasOauth {
            if p.acceptsToken {
                return "Sign-in not set up — paste a token"
            }
            return "OAuth client not configured on the relay"
        }
        if !p.hasOauth, p.acceptsToken {
            return "Connects with an API key"
        }
        if let err = cn?.lastError, !err.isEmpty, cn?.status != .connected {
            return err
        }
        if let cn, cn.status == .needsAuth {
            return ConnectorStatus.needsAuth.label
        }
        if let cn { return cn.status.label }
        return "Not connected"
    }
}

/// Settings → Devices (Oct 23): one card, a row per signed-in device — glyph,
/// name, kind tag, "Active now · added Oct 11" — this iPhone first and marked;
/// every other row has Sign out, which asks first and then revokes on the relay.
/// The current device signs out from the Settings tab, so you are never stranded.
struct DevicesPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @State private var pending: AuthDevice?
    @State private var errorDetailsOpen = false

    private var rows: [AuthDevice] {
        model.devices.sorted { a, b in a.current && !b.current }
    }

    var body: some View {
        SettingsPage(title: "Devices") {
            if let why = model.devicesError, model.devices.isEmpty {
                // The page's note: under the page header, before the first card.
                ErrorNote(id: "devices-error", error: why, detailsOpen: $errorDetailsOpen,
                          onTryAgain: { Task { await model.refreshDevices() } })
            } else if !model.devicesLoaded && model.devices.isEmpty {
                Text("Loading…").font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                    .frame(maxWidth: .infinity).padding(.top, 36)
            } else if model.devices.isEmpty {
                Text("No devices yet").font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                    .frame(maxWidth: .infinity).padding(.top, 36)
            } else {
                SettingsGroup(footer: "Every GitHub sign-in is a device. Signing one out ends its session at once; it can sign in again. This iPhone signs out from the Settings tab.") {
                    ForEach(rows) { d in
                        PaneRow(
                            leading: .tile(d.glyph),
                            title: d.displayName,
                            subtitle: d.subtitle(),
                            trailing: AnyView(trailing(d))
                        )
                        .accessibilityElement(children: .combine)
                        .accessibilityLabel(d.current ? "\(d.displayName), this iPhone" : d.displayName)
                    }
                }
            }
        }
        .task { await model.refreshDevices() }
        .confirmationDialog(
            AuthDevice.confirmQuestion(pending?.displayName ?? ""),
            isPresented: Binding(get: { pending != nil }, set: { if !$0 { pending = nil } }),
            titleVisibility: .visible
        ) {
            Button("Sign out", role: .destructive) {
                if let d = pending { model.revokeDevice(d.id) }
                pending = nil
            }
            Button("Cancel", role: .cancel) { pending = nil }
        }
    }

    @ViewBuilder private func trailing(_ d: AuthDevice) -> some View {
        // Tag and control on one line, never wrapping (the gate build broke
        // "Sign out" over two lines next to the "read-only" tag).
        HStack(spacing: 8) {
            if let tag = d.kindTag {
                Text(tag)
                    .font(theme.font(Theme.tiny))
                    .foregroundStyle(theme.muted)
                    .lineLimit(1).fixedSize()
                    .padding(.horizontal, 6).padding(.vertical, 2)
                    .background(Capsule().fill(theme.field)) // control-size: exception inlineChips
            }
            if d.current {
                Text("This iPhone").font(theme.font(Theme.small)).foregroundStyle(theme.muted).lineLimit(1).fixedSize()
            } else {
                Button { pending = d } label: {
                    // error-ui: exception destructive-action
                    Text("Sign out").font(theme.font(Theme.small)).foregroundStyle(theme.danger).lineLimit(1).fixedSize()
                }
                .buttonStyle(.borderless)
                .accessibilityLabel("Sign out \(d.displayName)")
            }
        }
        .fixedSize()
    }
}

struct NotificationsPage: View {
    @Environment(\.theme) private var theme
    @ObservedObject private var push = PushManager.shared

    var body: some View {
        SettingsPage(title: "Notifications") {
            SettingsGroup(footer: "A banner with the sender's picture when an agent finishes or a person writes, only while you are not looking. Permission lives in iOS Settings.") {
                SettingsValueRow("Allowed", value: permissionLabel)
                if let relay = push.relayPushEnabled {
                    SettingsValueRow("Push from the relay", value: relay ? "On" : "Off")
                }
                SettingsActionRow(title: "Open iOS Settings") {
                    if let url = URL(string: UIApplication.openNotificationSettingsURLString) {
                        UIApplication.shared.open(url)
                    }
                }
            }
        }
    }

    private var permissionLabel: String {
        switch push.status {
        case .authorized: return "Yes"
        case .provisional: return "Quietly"
        case .ephemeral: return "For now"
        case .denied: return "No"
        case .notDetermined: return "Not asked yet"
        @unknown default: return ""
        }
    }
}

/// Who may add me to people groups — stops random spam-group adds.
struct PrivacyPage: View {
    @EnvironmentObject private var model: AppModel
    @State private var error: UserError?
    @State private var errorDetailsOpen = false

    var body: some View {
        SettingsPage(title: "Privacy") {
            if let error {
                ErrorNote(id: "privacy-error", error: error, detailsOpen: $errorDetailsOpen)
            }
            SettingsGroup(
                header: "Groups",
                footer: "Controls who can put you in a group without asking. “People I message” means anyone you already have a direct chat with."
            ) {
                ForEach(GroupAddPolicy.allCases) { policy in
                    SettingsChoiceRow(
                        title: policy.label,
                        selected: model.groupAddPolicy == policy.rawValue
                    ) {
                        Task {
                            if let err = await model.updateGroupAddPolicy(policy) {
                                error = UserError.refused("save the privacy setting", err)
                            }
                        }
                    }
                }
            }
        }
    }
}

struct AppearancePage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme

    var body: some View {
        SettingsPage(title: "Appearance") {
            SettingsGroup(header: "Theme") {
                ForEach(Appearance.allCases) { a in
                    SettingsChoiceRow(title: a.label, selected: model.appearanceSetting == a.rawValue) {
                        model.appearanceSetting = a.rawValue
                    }
                }
            }
            SettingsGroup(header: "Surface", footer: "Glass blurs what scrolls under the header and the controls; Matte keeps them solid.") {
                ForEach(Surface.allCases) { s in
                    SettingsChoiceRow(title: s.label, selected: model.surfaceSetting == s.rawValue) {
                        model.surfaceSetting = s.rawValue
                    }
                }
            }
        }
    }
}

/// Q6: Telegram-style text size — a live two-bubble preview above a seven-step
/// slider. The slider writes the setting at once, so the whole app follows.
struct TextSizePage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme

    private var index: Int { TextScale.index(for: model.textScale) }

    var body: some View {
        SettingsPage(title: "Text Size") {
            SettingsGroup(header: "Preview") {
                VStack(alignment: .leading, spacing: 10) {
                    HStack {
                        Text("Hi! The launch note is ready — want me to post it, or tighten it first?")
                            .font(theme.font(Theme.body))
                            .lineSpacing(Theme.bodyLineSpacing)
                            .foregroundStyle(theme.text)
                            .padding(.horizontal, Theme.bubbleHPad).padding(.vertical, Theme.bubbleVPad)
                            .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleAssistant))
                            .frame(maxWidth: 280, alignment: .leading)
                        Spacer(minLength: 24)
                    }
                    HStack {
                        Spacer(minLength: 24)
                        Text("Tighten the first line, then post.")
                            .font(theme.font(Theme.body))
                            .lineSpacing(Theme.bodyLineSpacing)
                            .foregroundStyle(theme.userText)
                            .padding(.horizontal, Theme.bubbleHPad).padding(.vertical, Theme.bubbleVPad)
                            .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleUser))
                            .frame(maxWidth: 280, alignment: .trailing)
                    }
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .animation(.easeOut(duration: 0.15), value: index)
            }
            SettingsGroup(footer: "Changes the size of messages and labels everywhere in Arbos. \(TextScale.label(model.textScale))\(index == TextScale.defaultIndex ? "" : " · \(Int((model.textScale * 100).rounded()))%").") {
                HStack(spacing: 12) {
                    Text("A").font(.system(size: 13, weight: .medium)).foregroundStyle(theme.muted)
                    Slider(
                        value: Binding(get: { Double(index) },
                                       set: { v in
                                           let i = Int(v.rounded())
                                           let next = Double(TextScale.steps[max(0, min(TextScale.steps.count - 1, i))])
                                           if next != model.textScaleSetting { model.textScaleSetting = next; Haptics.pick() }
                                       }),
                        in: 0...Double(TextScale.steps.count - 1),
                        step: 1
                    )
                    .tint(theme.accent)
                    .accessibilityLabel("Text size")
                    .accessibilityValue(TextScale.label(model.textScale))
                    // The seven stops as notches on the track (Apple Display → Text Size / Telegram).
                    .background {
                        GeometryReader { geo in
                            let thumb: CGFloat = 13.5
                            let usable = geo.size.width - thumb * 2
                            ForEach(0..<TextScale.steps.count, id: \.self) { i in
                                Rectangle()
                                    .fill(i <= index ? theme.accent.opacity(0.9) : theme.muted.opacity(0.55))
                                    .frame(width: 2, height: 10)
                                    .position(x: thumb + usable * CGFloat(i) / CGFloat(TextScale.steps.count - 1), y: geo.size.height / 2)
                            }
                        }
                        .allowsHitTesting(false)
                    }
                    Text("A").font(.system(size: 24, weight: .medium)).foregroundStyle(theme.text)
                }
                .padding(.horizontal, 16)
                .frame(minHeight: 56)
            }
        }
    }
}

struct AboutPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme

    private var protocolValue: String {
        let st = model.sharedRoomStats
        var line = "room proto \(protoVersion) · Loro · core session · \(st.reconnects) reconnects · \(st.timeouts) timeouts"
        if let err = st.lastError { line += " · last error: \(err)" }
        return line
    }

    var body: some View {
        SettingsPage(title: "About") {
            SettingsGroup {
                SettingsValueRow("Version", value: SettingsView.versionLine)
                SettingsValueRow("Protocol", value: protocolValue)
            }
            // Continuous delivery (D85): every fix ships as a TestFlight build; the newest one is
            // one tap away, and TestFlight can install it by itself.
            SettingsGroup(footer: "New builds arrive through TestFlight within minutes of a fix. To install them without a tap, open TestFlight → Arbos and turn on Automatic Updates.") {
                if let update = model.availableUpdate {
                    Button { model.openTestFlight() } label: {
                        SettingsValueRow("Update available") {
                            Text("Build \(update.build) · Update").foregroundStyle(theme.primary)
                        }
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("about-update-row")
                } else {
                    SettingsValueRow("Updates", value: "Up to date")
                }
            }
        }
        .task { await model.checkForUpdate() }
    }
}

// MARK: - Profile name / handle

/// Choose or edit display name and @handle — onboarding after GitHub sign-in,
/// and Settings by tapping the profile name.
struct ProfileEditView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss

    /// After sign-in: CTA is "Continue"; sheet cannot be dismissed until saved.
    var onboarding: Bool = false

    @State private var name: String = ""
    @State private var handle: String = ""
    @State private var busy = false
    @State private var error: UserError?
    @State private var errorDetailsOpen = false
    @FocusState private var focus: Field?

    private enum Field { case name, handle }

    var body: some View {
        VStack(spacing: 0) {
            if onboarding {
                Capsule().fill(theme.separator).frame(width: 36, height: 5).padding(.top, 10)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    if onboarding {
                        Text("Your profile")
                            .font(theme.font(22, weight: .semibold))
                            .foregroundStyle(theme.text)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 12)
                        Text("Pick a name and handle people will see. You can change them later in Settings.")
                            .font(theme.font(Theme.small))
                            .foregroundStyle(theme.muted)
                            .multilineTextAlignment(.center)
                            .frame(maxWidth: .infinity)
                    }

                    // The form's note: above the fields (the shared element).
                    if let error {
                        ErrorNote(id: "profile-error", error: error, detailsOpen: $errorDetailsOpen,
                                  onTryAgain: { Task { await save() } })
                    }

                    nameField
                    handleField

                    Text("Handle: letters, numbers, and hyphens · up to 39 characters.")
                        .font(theme.font(Theme.tiny))
                        .foregroundStyle(theme.muted)
                }
                .padding(.horizontal, 24)
                .padding(.top, onboarding ? 8 : 20)
                .padding(.bottom, 24)
            }
            .scrollDismissesKeyboard(.interactively)
            // On short screens with the keyboard up the hint scrolls under the
            // button; fade that edge instead of slicing a line of text.
            .mask(
                VStack(spacing: 0) {
                    Color.black
                    LinearGradient(colors: [.black, .clear], startPoint: .top, endPoint: .bottom)
                        .frame(height: 20)
                }
            )
            .padding(.bottom, 6)

            Button {
                Task { await save() }
            } label: {
                Text(busy ? "Saving…" : (onboarding ? "Continue" : "Save"))
                    .font(theme.font(Theme.body, weight: .semibold))
                    .foregroundStyle(theme.onPrimary)
                    .frame(maxWidth: .infinity)
                    .frame(height: 50)
                    .background(Capsule().fill(theme.primary))
            }
            .buttonStyle(.plain)
            .disabled(!canSave || busy)
            .opacity(canSave && !busy ? 1 : 0.45)
            .padding(.horizontal, 24)
            .padding(.bottom, onboarding ? 28 : 16)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(theme.bg)
        .navigationTitle(onboarding ? "" : "Edit profile")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if !onboarding {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }.foregroundStyle(theme.text)
                }
            }
        }
        .interactiveDismissDisabled(onboarding)
        .onAppear {
            name = model.user?.name ?? ""
            handle = model.user?.login ?? ""
            focus = .name
        }
    }

    private var canSave: Bool {
        let h = handle.trimmingCharacters(in: .whitespacesAndNewlines)
        return !h.isEmpty && h.count <= 39 && Self.isValidHandle(h)
    }

    private var nameField: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Name").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
            TextField("", text: $name, prompt: Text("Display name").foregroundStyle(theme.placeholder))
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.text)
                .textContentType(.name)
                .textInputAutocapitalization(.words)
                .focused($focus, equals: .name)
                .submitLabel(.next)
                .onSubmit { focus = .handle }
                .controlField()
        }
    }

    private var handleField: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Handle").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
            HStack(spacing: 2) {
                Text("@").font(theme.font(Theme.body)).foregroundStyle(theme.muted)
                TextField("", text: $handle, prompt: Text("handle").foregroundStyle(theme.placeholder))
                    .font(theme.font(Theme.body))
                    .foregroundStyle(theme.text)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .focused($focus, equals: .handle)
                    .submitLabel(.done)
                    .onSubmit { Task { await save() } }
                    .onChange(of: handle) { _, new in
                        let cleaned = Self.sanitizeHandle(new)
                        if cleaned != new { handle = cleaned }
                    }
            }
            .controlField()
        }
    }

    private func save() async {
        guard canSave, !busy else { return }
        busy = true
        error = nil
        defer { busy = false }
        if let msg = await model.updateProfile(name: name, handle: handle) {
            error = UserError.refused("save your profile", msg)
            return
        }
        dismiss()
    }

    static func sanitizeHandle(_ raw: String) -> String {
        let trimmed = raw.trimmingCharacters(in: CharacterSet(charactersIn: "@"))
        let allowed = trimmed.filter { c in
            c.isASCII && (c.isLetter || c.isNumber || c == "-")
        }
        return String(allowed.prefix(39))
    }

    static func isValidHandle(_ handle: String) -> Bool {
        !handle.isEmpty
            && handle.count <= 39
            && !handle.hasPrefix("-")
            && !handle.hasSuffix("-")
            && handle.allSatisfy { c in c.isASCII && (c.isLetter || c.isNumber || c == "-") }
    }
}

#if DEBUG
/// Self-hosters point the app at their own relay here (debug builds only, as before).
struct RelayPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @State private var relayUrl = ""

    var body: some View {
        SettingsPage(title: "Relay") {
            SettingsGroup(footer: "Switching relays signs this device out of the current one; sessions stay in the Keychain.") {
                HStack {
                    TextField("", text: $relayUrl, prompt: Text("https://relay.example.com").foregroundStyle(theme.placeholder))
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(theme.font(Theme.body))
                        .foregroundStyle(theme.text)
                        .onSubmit(applyRelay)
                }
                .padding(.horizontal, 16)
                .frame(minHeight: SettingsRow.height)
                if RelayEndpoint(relayUrl).httpUrl != model.relayUrl {
                    SettingsActionRow(title: "Switch relay", action: applyRelay)
                }
            }
            SettingsGroup {
                SettingsValueRow("Device") {
                    Text(model.deviceId).font(theme.font(12, design: .monospaced)).lineLimit(1).truncationMode(.middle)
                }
                SettingsValueRow("Link", value: linkLabel)
            }
        }
        .onAppear { relayUrl = model.relayUrl }
    }

    private var linkLabel: String {
        switch model.link {
        case .connecting: return "Connecting…"
        case .live: return "Live"
        case .reconnecting: return "Reconnecting…"
        }
    }

    private func applyRelay() {
        let trimmed = relayUrl.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return }
        model.setRelayUrl(trimmed)
        dismiss()
    }
}
#endif

// MARK: - Goals (long-running objectives)

struct GoalsPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @State private var showNew = false

    private var open: [GoalItem] { model.goals.filter(\.isOpen) }
    private var closed: [GoalItem] { model.goals.filter { !$0.isOpen } }

    var body: some View {
        SettingsPage(title: "Planned tasks") {
            if model.goals.isEmpty {
                Text("Long-running work agents track across days. Ask an agent to make something a goal, or start one here.")
                    .font(theme.font(Theme.small))
                    .foregroundStyle(theme.muted)
                    .padding(.horizontal, 4)
                    .padding(.bottom, 8)
            }
            if !open.isEmpty {
                SettingsGroup(header: "Active") {
                    ForEach(open) { g in
                        NavigationLink { GoalDetailPage(goalId: g.id) } label: {
                            GoalRow(goal: g)
                        }
                    }
                }
            }
            if !closed.isEmpty {
                SettingsGroup(header: "Paused & done") {
                    ForEach(closed) { g in
                        NavigationLink { GoalDetailPage(goalId: g.id) } label: {
                            GoalRow(goal: g)
                        }
                    }
                }
            }
            SettingsGroup {
                Button { showNew = true } label: {
                    SettingsRow(icon: "plus", title: "New goal")
                }
                .buttonStyle(.plain)
            }
        }
        .task { await model.refreshGoals() }
        .sheet(isPresented: $showNew) {
            NavigationStack {
                NewGoalSheet().environmentObject(model).environment(\.theme, theme)
            }
        }
    }
}

struct GoalRow: View {
    @Environment(\.theme) private var theme
    let goal: GoalItem

    var body: some View {
        HStack(spacing: 14) {
            RoundedRectangle(cornerRadius: 7.5, style: .continuous)
                .fill(statusColor)
                .frame(width: SettingsRow.tile, height: SettingsRow.tile)
                .overlay(
                    Image(systemName: "flag.fill")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(.white)
                )
            VStack(alignment: .leading, spacing: 2) {
                Text(goal.title)
                    .font(theme.font(Theme.body))
                    .foregroundStyle(theme.text)
                    .lineLimit(2)
                Text(subtitle)
                    .font(theme.font(Theme.tiny))
                    .foregroundStyle(theme.muted)
                    .lineLimit(2)
            }
            Spacer(minLength: 8)
            Image(systemName: "chevron.right")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(theme.placeholder)
        }
        .padding(.horizontal, 16)
        .frame(minHeight: 56)
        .contentShape(Rectangle())
    }

    private var subtitle: String {
        var parts: [String] = [goal.statusLabel]
        if !goal.nextStep.isEmpty { parts.append(goal.nextStep) }
        if let moved = goal.lastMovedAt {
            parts.append(SecretsText.ago(moved))
        }
        return parts.joined(separator: " · ")
    }

    private var statusColor: Color {
        switch goal.status {
        case "blocked": return theme.danger
        case "waiting": return theme.warn
        case "done": return theme.online
        case "paused": return theme.muted
        default: return theme.primary
        }
    }
}

struct GoalDetailPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let goalId: String

    private var goal: GoalItem? { model.goals.first { $0.id == goalId } }

    var body: some View {
        Group {
            if let goal {
                SettingsPage(title: goal.title) {
                    SettingsGroup(footer: goal.outcome.isEmpty ? nil : goal.outcome) {
                        SettingsValueRow("Status", value: goal.statusLabel)
                        if let name = goal.projectName ?? model.project(goal.projectId)?.name {
                            SettingsValueRow("Agent", value: name)
                        }
                        SettingsValueRow("Next step", value: goal.nextStep.isEmpty ? "—" : goal.nextStep)
                        SettingsValueRow("Check-in", value: goal.checkInLabel)
                        SettingsValueRow("Autonomy", value: goal.autonomyLabel)
                        if let q = goal.waitingQuestion, !q.isEmpty {
                            SettingsValueRow("Waiting", value: q)
                        }
                    }
                    if !goal.plan.isEmpty {
                        SettingsGroup(header: "Plan") {
                            ForEach(goal.plan) { step in
                                SettingsValueRow(step.title, value: step.status)
                            }
                        }
                    }
                    if !goal.progressLog.isEmpty {
                        SettingsGroup(header: "Progress") {
                            ForEach(goal.progressLog.reversed()) { entry in
                                VStack(alignment: .leading, spacing: 4) {
                                    Text(entry.text)
                                        .font(theme.font(Theme.small))
                                        .foregroundStyle(theme.text)
                                    if entry.at > 0 {
                                        Text(SecretsText.ago(entry.at))
                                            .font(theme.font(Theme.tiny))
                                            .foregroundStyle(theme.muted)
                                    }
                                }
                                .padding(.horizontal, 16)
                                .padding(.vertical, 12)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            }
                        }
                    }
                    SettingsGroup {
                        if goal.status != "paused" && goal.status != "done" {
                            Button { model.setGoalStatus(goal, status: "paused") } label: {
                                SettingsRow(icon: "pause.fill", title: "Pause")
                            }
                            .buttonStyle(.plain)
                        }
                        if goal.status == "paused" || goal.status == "done" {
                            Button { model.setGoalStatus(goal, status: "active") } label: {
                                SettingsRow(icon: "play.fill", title: "Resume")
                            }
                            .buttonStyle(.plain)
                        }
                        if goal.status != "done" {
                            Button { model.setGoalStatus(goal, status: "done") } label: {
                                SettingsRow(icon: "checkmark", title: "Mark done")
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    SettingsGroup(header: "Check-in") {
                        ForEach(GoalCheckInPreset.all) { preset in
                            Button {
                                var body: [String: Any] = [
                                    "checkInCron": preset.cron ?? "",
                                ]
                                if preset.cron != nil {
                                    body["checkInTz"] = TimeZone.current.identifier
                                }
                                model.patchGoal(goal, body)
                            } label: {
                                SettingsRow(
                                    icon: "clock",
                                    title: preset.label,
                                    value: goal.checkInCron == preset.cron || (preset.cron == nil && goal.checkInCron == nil) ? "Selected" : nil
                                )
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    SettingsGroup(header: "Autonomy", footer: "How much the agent may do on check-in without asking. Uses the same approval cards as the rest of Arbos.") {
                        ForEach(GoalAutonomyPreset.all) { preset in
                            Button { model.patchGoal(goal, ["autonomy": preset.id]) } label: {
                                SettingsRow(
                                    icon: "hand.raised.fill",
                                    title: preset.label,
                                    value: goal.autonomy == preset.id ? "Selected" : nil
                                )
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    SettingsGroup {
                        Button(role: .destructive) { model.deleteGoal(goal) } label: {
                            Text("Delete goal")
                                .font(theme.font(Theme.body))
                                // error-ui: exception destructive-action
                                .foregroundStyle(theme.danger)
                                .frame(maxWidth: .infinity, minHeight: SettingsRow.height)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            } else {
                SettingsPage(title: "Goal") {
                    Text("This goal is gone.")
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                }
            }
        }
        .task { await model.refreshGoals() }
    }
}

private struct GoalCheckInPreset: Identifiable {
    let id: String
    let label: String
    let cron: String?
    static let all: [GoalCheckInPreset] = [
        .init(id: "daily", label: "Every day", cron: "0 9 * * *"),
        .init(id: "weekdays", label: "Weekdays", cron: "0 9 * * 1-5"),
        .init(id: "weekly", label: "Weekly", cron: "0 9 * * 1"),
        .init(id: "manual", label: "Manual only", cron: nil),
    ]
}

private struct GoalAutonomyPreset: Identifiable {
    let id: String
    let label: String
    static let all: [GoalAutonomyPreset] = [
        .init(id: "ask", label: "Ask first"),
        .init(id: "suggest", label: "Suggest first"),
        .init(id: "act", label: "Act freely"),
    ]
}

struct NewGoalSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @State private var title = ""
    @State private var outcome = ""
    @State private var nextStep = ""
    @State private var projectId: Id = ""
    @State private var checkIn = GoalCheckInPreset.all[0]
    @State private var autonomy = GoalAutonomyPreset.all[0]

    private var projects: [Project] { model.projects.filter { !$0.shared } }

    var body: some View {
        SettingsPage(title: "New goal", onBack: { dismiss() }) {
            SettingsGroup {
                SecretField(label: "Title") {
                    SecretTextField(placeholder: "What are you working toward?", text: $title, capitalization: .sentences)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
                SecretField(label: "Desired outcome") {
                    SecretTextField(placeholder: "What done looks like", text: $outcome, multiline: true, capitalization: .sentences)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
                SecretField(label: "Next step") {
                    SecretTextField(placeholder: "First thing to do", text: $nextStep, capitalization: .sentences)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
            }
            if !projects.isEmpty {
                SettingsGroup(header: "Agent") {
                    ForEach(projects) { p in
                        Button {
                            projectId = p.id
                        } label: {
                            SettingsRow(
                                icon: "person.crop.circle",
                                title: p.name,
                                value: projectId == p.id ? "Selected" : nil
                            )
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            SettingsGroup(header: "Check-in") {
                ForEach(GoalCheckInPreset.all) { preset in
                    Button { checkIn = preset } label: {
                        SettingsRow(icon: "clock", title: preset.label, value: checkIn.id == preset.id ? "Selected" : nil)
                    }
                    .buttonStyle(.plain)
                }
            }
            SettingsGroup(header: "Autonomy") {
                ForEach(GoalAutonomyPreset.all) { preset in
                    Button { autonomy = preset } label: {
                        SettingsRow(icon: "hand.raised.fill", title: preset.label, value: autonomy.id == preset.id ? "Selected" : nil)
                    }
                    .buttonStyle(.plain)
                }
            }
            SettingsGroup {
                Button {
                    let pid = projectId.isEmpty ? (projects.first?.id ?? "") : projectId
                    guard !pid.isEmpty, !title.trimmingCharacters(in: .whitespaces).isEmpty else { return }
                    model.createGoal(
                        projectId: pid,
                        title: title.trimmingCharacters(in: .whitespaces),
                        outcome: outcome.trimmingCharacters(in: .whitespacesAndNewlines),
                        nextStep: nextStep.trimmingCharacters(in: .whitespacesAndNewlines),
                        checkInCron: checkIn.cron,
                        autonomy: autonomy.id
                    )
                    dismiss()
                } label: {
                    Text("Create goal")
                        .font(theme.font(Theme.body, weight: .semibold))
                        .foregroundStyle(theme.onPrimary)
                        .frame(maxWidth: .infinity, minHeight: SettingsRow.height)
                        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(theme.primary))
                        .padding(.horizontal, 16)
                        .padding(.vertical, 8)
                }
                .buttonStyle(.plain)
                .disabled(title.trimmingCharacters(in: .whitespaces).isEmpty || projects.isEmpty)
            }
        }
        .onAppear {
            if projectId.isEmpty { projectId = projects.first?.id ?? "" }
        }
    }
}
