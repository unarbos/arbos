// Simulator-only hooks so a headless `simctl launch` can land on a given
// screen for screenshots. Compiled out of Release builds.

import Foundation

enum DebugHooks {
    static var openProject: Id? { value("ARBOS_OPEN_PROJECT") }
    static var openChat: Id? { value("ARBOS_OPEN_CHAT") }
    /// Live relay rig: open this people chat (DM or group) once the list has it.
    static var openPeople: Id? { value("ARBOS_OPEN_PEOPLE") }
    /// D88: `<name or id>[:list|:home|:page|:page-settings|:#channel]` — land in a server (screenshot rig).
    static var openServer: String? { value("ARBOS_OPEN_SERVER") }
    /// With `ARBOS_OPEN_SERVER=…:panel`: the server page's tab to land on (Channels | Members | Settings).
    static var panelTab: String? { value("ARBOS_PANEL_TAB") }
    static var autoSend: String? { value("ARBOS_AUTO_SEND") }
    static var pairCode: String? { value("ARBOS_PAIR_CODE") }
    static var fixture: Bool { value("ARBOS_FIXTURE") == "1" }
    /// D89 waitlist card and Machine rows: the fixture account's place in line.
    static var fixtureWaitlist: Int? { value("ARBOS_FIXTURE_WAITLIST").flatMap { Int($0) } }
    /// Seed the connected 1Password vault and the connect flow (`features.vault_connect` on).
    /// Default off, like the shipped relay flag.
    static var fixtureVault: Bool { value("ARBOS_FIXTURE_VAULT") == "1" }
    /// With `ARBOS_OPEN_CHAT`: that chat ends with a several-block agent reply (#1389 rigs).
    static var fixtureLongReply: Bool { value("ARBOS_FIXTURE_LONG_REPLY") == "1" }
    /// Presence-row rig (typing-indicator-left-2026-10-23): n agents working in the fixture group.
    static var fixtureWorking: Int { value("ARBOS_FIXTURE_WORKING").flatMap { Int($0) } ?? 0 }
    /// Presence-row rig: `1` (the chat's other person) or comma-separated handles typing.
    static var fixtureTyping: String? { value("ARBOS_FIXTURE_TYPING").flatMap { $0.isEmpty || $0 == "0" ? nil : $0 } }
    /// Join a call seen via presence automatically (two-simulator relay tests).
    static var autoJoinCall: Bool { value("ARBOS_AUTO_JOIN") == "1" }
    /// Accept an incoming call automatically (two-simulator relay tests).
    static var autoAcceptCall: Bool { value("ARBOS_AUTO_ACCEPT") == "1" }
    /// Headless simulators have no audio device: WAV files dropped into this directory are sent
    /// as the microphone (LiveKit manual-rendering mode), in file-name order, once each.
    static var micDir: String? { value("ARBOS_MIC_DIR") }
    /// With `ARBOS_MIC_DIR`: also switch the audio engine to manual rendering (no device at all).
    static var micManual: Bool { value("ARBOS_MIC_MANUAL") == "1" }
    /// Join a LiveKit room directly (dev server + locally minted token), bypassing the relay.
    static var callUrl: String? { value("ARBOS_CALL_URL") }
    static var callToken: String? { value("ARBOS_CALL_TOKEN") }
    static var callKind: String? { value("ARBOS_CALL_KIND") }
    /// Simulator has no camera: pretend ours is on and the agent reports seeing it (§11 badge screenshots).
    static var fakeVision: Bool { value("ARBOS_FAKE_VISION") == "1" }
    /// Text to put in the feedback sheet and send as soon as it opens (endpoint check).
    static var autoFeedback: String? { value("ARBOS_AUTO_FEEDBACK") }
    /// Fixture machine reports `always_on` (D73 policy screens).
    static var alwaysOn: Bool { value("ARBOS_ALWAYS_ON") == "1" }
    /// keyboard | settings | voice | threads | model | profile | member — opened on the chat screen
    /// (`member` = the group page with its first other member pushed over it).
    /// `onboarding` shows the D82 first-login screen frozen on `ARBOS_ONBOARDING_STAGE` (1|2|3|fail).
    static var show: String? { value("ARBOS_SHOW") }
    /// How long after the chat appears `ARBOS_SHOW` fires (ms, default 600): a loaded rig Mac
    /// needs more before a sheet can be presented.
    static var showDelayMs: Int { value("ARBOS_SHOW_DELAY_MS").flatMap(Int.init) ?? 600 }
    /// With `ARBOS_SHOW=newagent`: prefill the Create agent name (the "named" state on the rig).
    static var newAgentName: String? { value("ARBOS_NEW_AGENT_NAME") }
    static var onboardingStage: String? { value("ARBOS_ONBOARDING_STAGE") }
    static var onboardingAutoplay: Bool { value("ARBOS_ONBOARDING_AUTOPLAY") == "1" }
    /// With `ARBOS_SHOW=onboarding`: also present the name/handle sheet over the terminal (rig).
    static var onboardingProfile: Bool { value("ARBOS_ONBOARDING_PROFILE") == "1" }
    /// With `ARBOS_SHOW=settings`: the Settings page to land on — appearance, textsize, connectors, secrets, machine, credits, notifications, about, relay.
    static var settingsPage: String? { value("ARBOS_SETTINGS_PAGE") }
    /// With a Settings page open: scroll to the card with this id (e.g. `people` on a Secret page).
    static var settingsScroll: String? { value("ARBOS_SETTINGS_SCROLL") }
    /// On a Secret page: open a sheet at once — `people` (Give a person access), `person` (the
    /// first People row's note sheet).
    static var secretSheet: String? { value("ARBOS_SECRET_SHEET") }
    /// Text Size factor for screenshot sweeps (one of TextScale.steps, e.g. 0.85 / 1.3).
    static var textScale: Double? { value("ARBOS_TEXT_SCALE").flatMap(Double.init) }
    /// `system | light | dark`: the theme for one launch, over the saved choice and the
    /// D90 Dark default (the rig shoots light + dark by toggling the simulator with `system`).
    static var appearance: Appearance? { value("ARBOS_APPEARANCE").flatMap(Appearance.init(rawValue:)) }
    /// `matte | glass`: the surface for one launch, same precedence.
    static var surface: Surface? { value("ARBOS_SURFACE").flatMap(Surface.init(rawValue:)) }
    /// `offline | relay-down | reconnecting | syncing | restart-soon`: the open chat's link state for the
    /// header's connection pill; `ARBOS_LINK_CLEARS_MS=<ms>` sets it back to live after that long.
    static var link: LinkState? {
        switch value("ARBOS_LINK") {
        case "offline": return .offline
        case "relay-down": return .relayDown
        case "reconnecting": return .reconnecting
        case "syncing": return .syncing
        case "restart-soon": return .restartSoon
        default: return nil
        }
    }
    static var linkClearsMs: Int? { value("ARBOS_LINK_CLEARS_MS").flatMap { Int($0) } }
    /// Long-chat performance rig (long-chat-perf, 2026-10-23): pad the fixture group
    /// "Launch week" with this many synthetic rows — mixed people / agent prose with
    /// markdown, relay system lines, replies, attachments.
    static var fixtureLongRows: Int? { value("ARBOS_FIXTURE_LONG_ROWS").flatMap { Int($0) } }
    /// Log frame pacing (dropped frames, worst frame, time over budget) and memory every
    /// 2 s to `com.unarbos.arbos.ios` / `perf` for the duration of the launch.
    static var perfLog: Bool { value("ARBOS_PERF_LOG") == "1" }
    /// Scripted tour for the screen recording (fixture only).
    static var demo: Bool { value("ARBOS_DEMO") == "1" }
    /// Scripted tour against the live relay: sends ARBOS_AUTO_SEND, waits for the reply, tours the sheets.
    static var liveDemo: Bool { value("ARBOS_DEMO") == "live" }

    private static func value(_ key: String) -> String? {
        #if DEBUG
        let v = ProcessInfo.processInfo.environment[key] ?? ""
        return v.isEmpty ? nil : v
        #else
        return nil
        #endif
    }
}
