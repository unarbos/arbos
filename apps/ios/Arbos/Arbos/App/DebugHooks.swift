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
    /// Home search opened with this query (search-name-matching rig).
    static var search: String? { value("ARBOS_SEARCH").flatMap { $0.isEmpty ? nil : $0 } }
    /// With `ARBOS_SEARCH`: open the n-th result row (1-based, list order) once the rows are up.
    static var searchOpen: Int? { value("ARBOS_SEARCH_OPEN").flatMap { Int($0) } }
    /// Home list segment selected at launch: all | agents | people | servers (screenshot rig).
    static var homeFilter: String? { value("ARBOS_HOME_FILTER") }
    /// With `ARBOS_OPEN_SERVER=…:panel`: the server page's tab to land on (Channels | Members | Settings).
    static var panelTab: String? { value("ARBOS_PANEL_TAB") }
    /// With the agent pane open: scroll so the block with this id is in view (`permissions`,
    /// `profile`) — the Details blocks below the fold on the rig (#1544's Shell approvals).
    static var paneScroll: String? { value("ARBOS_PANE_SCROLL") }
    /// Screenshot rig ([feedback iphone-list-top-fade-2026-10-26]): scroll a list (Home, search
    /// results, server channels) so its row with this index sits at the top — rows under the bar.
    static var listScroll: Int? { value("ARBOS_LIST_SCROLL").flatMap { Int($0) } }
    static var autoSend: String? { value("ARBOS_AUTO_SEND") }
    /// Rig (#1596): text put in the composer of the opened chat at appear, caret at the end —
    /// `@s` leaves the mention picker open, `@sam deck ready?` shows the resolved chip.
    static var composerDraft: String? { value("ARBOS_COMPOSER_DRAFT").flatMap { $0.isEmpty ? nil : $0 } }
    /// Rig (#1596): print each people-chat send's text and `mentions` ids to the console.
    static var traceSends: Bool { value("ARBOS_TRACE_SENDS") == "1" }
    static var pairCode: String? { value("ARBOS_PAIR_CODE") }
    /// Rig: a test-token sign-in skips the first-login stages (the account exists already).
    static var skipFirstLogin: Bool { value("ARBOS_SKIP_FIRST_LOGIN") == "1" }
    static var fixture: Bool { value("ARBOS_FIXTURE") == "1" }
    /// D89 waitlist card and Machine rows: the fixture account's place in line.
    static var fixtureWaitlist: Int? { value("ARBOS_FIXTURE_WAITLIST").flatMap { Int($0) } }
    /// `ARBOS_FIXTURE_SERVERS=1`: the fixture lists three servers (the home list's Servers segment).
    static var fixtureServers: Bool { value("ARBOS_FIXTURE_SERVERS") == "1" }
    /// `member` | `admin`: my role in the fixture group "Launch week" (owner by default).
    static var fixtureGroupRole: String? {
        value("ARBOS_FIXTURE_GROUP_ROLE").flatMap { ["member", "admin"].contains($0) ? $0 : nil }
    }
    /// Seed the connected 1Password vault and the connect flow (`features.vault_connect` on).
    /// Default off, like the shipped relay flag.
    static var fixtureVault: Bool { value("ARBOS_FIXTURE_VAULT") == "1" }
    /// #1544 rig: the Comms chat ends with a destructive-shell card (four standing rows).
    static var fixtureShellCard: Bool { value("ARBOS_FIXTURE_SHELL_CARD") == "1" }
    /// Secret hand-off rig: the Comms chat ends with the request card (`offer` / `saved`), the
    /// "save this" ghost row (`ghost`) or the offer card for a pasted key (`detect`).
    static var fixtureSecretHandoff: String? { value("ARBOS_FIXTURE_SECRET_HANDOFF") }
    /// With `ARBOS_OPEN_CHAT`: that chat ends with a several-block agent reply (#1389 rigs).
    static var fixtureLongReply: Bool { value("ARBOS_FIXTURE_LONG_REPLY") == "1" }
    /// Presence-row rig (typing-indicator-left-2026-10-23): n agents working in the fixture group.
    static var fixtureWorking: Int { value("ARBOS_FIXTURE_WORKING").flatMap { Int($0) } ?? 0 }
    /// `ARBOS_FIXTURE_MACHINE=near-limit`: the box report says disk 92 % and memory 88 % used
    /// (Settings › Machine's warning state on the rig).
    static var fixtureMachineNearLimit: Bool { value("ARBOS_FIXTURE_MACHINE") == "near-limit" }
    /// Tasks-chip rig (duplex §2.4): the first n of three background jobs open in the opened
    /// agent chat — 1 running with a step, 2 running with no step yet, 3 queued behind a lock.
    static var fixtureJobs: Int { min(3, value("ARBOS_FIXTURE_JOBS").flatMap { Int($0) } ?? 0) }
    /// `ARBOS_FIXTURE_READS=1` ([feedback library-chat-only-ios-2026-10-25]): the opened agent
    /// chat ends like Bubble's did — a hidden job row that read app.rs and a reply naming ten
    /// files and a docs page; none of it belongs in the Library.
    static var fixtureReads: Bool { value("ARBOS_FIXTURE_READS") == "1" }
    /// Job-tab rig: the first seeded job's document is finished (result delivered).
    static var fixtureRolloutDone: Bool { value("ARBOS_FIXTURE_ROLLOUT_DONE") == "1" }
    /// #1592 rig: the fixture group holds someone else's agent ("TestAgent", run by @sami —
    /// in the roster, not in my projects) with one bubble; with `ARBOS_FIXTURE_WORKING` it works.
    static var fixtureForeignAgent: Bool { value("ARBOS_FIXTURE_FOREIGN_AGENT") == "1" }
    /// `ARBOS_FIXTURE_BLOCKED=<handle,…>`: these fixture people are on my block list, as
    /// `GET /blocks` would hand it over (the Mac fixture's hook of the same name; blocked-state
    /// rig: the DM's bar and tag, the group's folded rows, the member list's tags).
    static var fixtureBlocked: Set<String> {
        Set((value("ARBOS_FIXTURE_BLOCKED") ?? "").split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty })
    }
    /// `ARBOS_FIXTURE_REPLY_DM=1`: rows in the fixture DM that answer messages in other chats
    /// (Reply in DM rig, #1836).
    static var fixtureReplyDM: Bool { value("ARBOS_FIXTURE_REPLY_DM") == "1" }
    /// `ARBOS_FIXTURE_CHIPS=1` ([feedback mention-chip-highlight-2026-10-28]): the fixture group
    /// ends with a member's and my own bubble, each with an agent and a person chip (mine has one
    /// inside a code span).
    static var fixtureChips: Bool { value("ARBOS_FIXTURE_CHIPS") == "1" }
    /// `ARBOS_FIXTURE_LINKS=1`: four links from unconst in the fixture DM, for the Links tab's
    /// tiles ([feedback link-icons-ios-tile-2026-10-27]).
    static var fixtureLinks: Bool { value("ARBOS_FIXTURE_LINKS") == "1" }
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
    /// (`member` = the group page with its first other member pushed over it; `actions` = the
    /// long-press overlay on the newest bubble, `keyboard-actions` = the same with the composer
    /// keyboard up first).
    /// `onboarding` shows the D82 first-login screen frozen on `ARBOS_ONBOARDING_STAGE` (1|2|3|fail).
    static var show: String? { value("ARBOS_SHOW") }
    /// With the long-press overlay open: stroke every menu row's and capsule cell's hit rect
    /// (iphone-bubble-menu-hit-area rig — the shot shows what a tap reaches).
    static var hitRects: Bool { value("ARBOS_HIT_RECTS") == "1" }
    /// How long after the chat appears `ARBOS_SHOW` fires (ms, default 600): a loaded rig Mac
    /// needs more before a sheet can be presented.
    static var showDelayMs: Int { value("ARBOS_SHOW_DELAY_MS").flatMap(Int.init) ?? 600 }
    /// With `ARBOS_SHOW=newagent`: prefill the Create agent name (the "named" state on the rig).
    static var newAgentName: String? { value("ARBOS_NEW_AGENT_NAME") }
    /// With a Create screen open: `upload | generate[:<prompt>]` picks the picture option (and
    /// types the prompt).
    static var createPicture: String? { value("ARBOS_CREATE_PICTURE") }
    static var onboardingStage: String? { value("ARBOS_ONBOARDING_STAGE") }
    static var onboardingAutoplay: Bool { value("ARBOS_ONBOARDING_AUTOPLAY") == "1" }
    /// With `ARBOS_SHOW=onboarding`: also present the name/handle sheet over the terminal (rig).
    static var onboardingProfile: Bool { value("ARBOS_ONBOARDING_PROFILE") == "1" }
    /// With `ARBOS_SHOW=settings`: the Settings page to land on — appearance, textsize, connectors, secrets, machine, credits, notifications, about, relay;
    /// `avatar[-generate|-upload]` opens the avatar picker over my picture.
    static var settingsPage: String? { value("ARBOS_SETTINGS_PAGE") }
    /// With the avatar picker open on Generate: type this prompt and press Generate (a shot
    /// shows prompt → result).
    static var avatarPrompt: String? { value("ARBOS_AVATAR_PROMPT") }
    /// `ARBOS_VIEW_PICTURE=1`: the first viewable hero picture on screen opens the media viewer
    /// (the picker's View does the same with `ARBOS_SETTINGS_PAGE=avatar-view`).
    static var viewPicture: Bool { value("ARBOS_VIEW_PICTURE") == "1" }
    /// `ARBOS_VIEWER_CLOSE_MS=<ms>`: the media viewer closes itself through its own Close path
    /// that long after it appears (a shot of what is underneath afterwards).
    static var viewerCloseMs: Int? { value("ARBOS_VIEWER_CLOSE_MS").flatMap(Int.init) }
    /// With a Settings page open: scroll to the card with this id (e.g. `people` on a Secret page).
    static var settingsScroll: String? { value("ARBOS_SETTINGS_SCROLL") }
    /// `ARBOS_RIG_AGENT_PICKED=<name>,<name>`: a Secrets form's agent picker starts with these
    /// agents as chips ([feedback secrets-agent-picker-ios-2026-10-26] shots).
    static var agentPickerPicked: [String] {
        (value("ARBOS_RIG_AGENT_PICKED") ?? "").split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    }
    /// `ARBOS_RIG_PICK=<agent name>,@<handle>,…`: the Add members picker pre-picks these rows
    /// once it is up; `ARBOS_RIG_AUTO_ADD=<ms>` then runs Add that long after
    /// ([feedback iphone-add-members-2026-10-26] live proof on a scratch group).
    static var rigPick: [String]? {
        let list = (value("ARBOS_RIG_PICK") ?? "").split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        return list.isEmpty ? nil : list
    }
    static var rigAutoAddMs: Int? { value("ARBOS_RIG_AUTO_ADD").flatMap(Int.init) }
    /// `ARBOS_RIG_AGENT_PICKER=<query>`: the picker's "Add agent" sheet starts open with that
    /// query typed (empty = open, nothing typed).
    static var agentPickerQuery: String? { value("ARBOS_RIG_AGENT_PICKER") }
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
    /// `translucent`: the Glass pills as a no-blur translucent fill instead of the system glass
    /// (mock for Jacob, [feedback iphone-composer-glass-2026-10-28] rig).
    static var pillStyle: String? { value("ARBOS_PILL_STYLE") }
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
    static let perfLog: Bool = value("ARBOS_PERF_LOG") == "1"
    /// Rig (chart bubbles): print every chart-plot gesture event (`[arbos chart] …`) so a
    /// simulator run can tell a touch that never reached the plot from a readout that did not draw.
    static let traceChart: Bool = value("ARBOS_TRACE_CHART") == "1"
    /// Seed the fixture group from a Loro snapshot file (a relay export of a real chat).
    static var fixtureDoc: String? { value("ARBOS_FIXTURE_DOC") }
    /// Live-path replay (iphone-open-scroll-block): `<checkpoint path>|<rows.hex path>`. The
    /// fixture group gets a real room (`ensureRoom`, store-restore shape) from the checkpoint,
    /// then the rows are imported one by one on the main actor the way the relay socket
    /// delivers them — the production doc → messages → rows path, without a network.
    static var perfReplay: (checkpoint: String, rows: String)? {
        guard let v = value("ARBOS_PERF_REPLAY"), let bar = v.firstIndex(of: "|") else { return nil }
        return (String(v[..<bar]), String(v[v.index(after: bar)...]))
    }
    /// How many rooms replay at once (the same doc under different chat ids): a launch
    /// with many busy chats catching up together.
    static var perfReplayRooms: Int { Int(value("ARBOS_PERF_REPLAY_ROOMS") ?? "") ?? 1 }
    /// Milliseconds between replayed rows (0 = as fast as the run loop allows).
    static var perfReplayGapMs: Int { Int(value("ARBOS_PERF_REPLAY_GAP_MS") ?? "") ?? 5 }
    /// Replay as live rows (room caught up) rather than as a catch-up.
    static var perfReplayLive: Bool { value("ARBOS_PERF_REPLAY_LIVE") == "1" }
    /// Delay before the replay starts, after the chat screen appears (ms).
    static var perfReplayAfterMs: Int { Int(value("ARBOS_PERF_REPLAY_AFTER_MS") ?? "") ?? 1500 }
    /// Print SwiftUI's change reasons (`_printChanges`) for the chat screen, the row
    /// list and each bubble (stdout; launch with `--stdout`).
    static let perfChanges: Bool = value("ARBOS_PERF_CHANGES") == "1"
    /// Perf rig drivers, run a few seconds after the chat opens: `scroll` pages the
    /// transcript to its top and back, `type` appends 40 keystrokes to the composer,
    /// `typing` fakes a member's typing presence every second. Comma-separated.
    static var perfDrive: Set<String> { Set((value("ARBOS_PERF_DRIVE") ?? "").split(separator: ",").map(String.init)) }
    /// With `ARBOS_PERF_DRIVE=switch`: the other people chat to alternate with (default the fixture DM).
    static var perfSwitchTo: String? { value("ARBOS_PERF_SWITCH_TO") }
    /// Scroll rig (iphone-scroll-perf, 2026-10-27): log every touch sequence on the chat
    /// screen — touch-down → scroll pan began / first content-offset change (latency), and
    /// pans that moved the finger but never moved the transcript ("dropped") — to the `perf`
    /// log, plus a one-time dump of the gesture-recognizer graph under the first touch.
    static let perfTouch: Bool = value("ARBOS_PERF_TOUCH") == "1"
    /// Scroll rig A/B: comma-separated gestures left out of the chat screen for this launch —
    /// `time` (transcript swipe-left time peek), `back` (the full-screen swipe-right back; the
    /// system edge pop is never off), `longpress` (bubble long-press menu).
    static let gesturesOff: Set<String> = Set((value("ARBOS_PERF_GESTURES_OFF") ?? "").split(separator: ",").map(String.init))
    /// With `ARBOS_FIXTURE_LONG_ROWS`: an Arbonites-shaped history — code cards, chart bubbles,
    /// @mentions, reactions and real (generated) pictures on top of the prose / markdown rows.
    static var fixtureLongRich: Bool { value("ARBOS_FIXTURE_LONG_RICH") == "1" }
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
