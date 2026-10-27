// Call screens, Grok Bot style: title at the top, a big rounded card with
// the other person (their video when the camera is on, else their avatar),
// name + status, glass controls and a red Hang up. Groups get a 2-column
// grid. `IncomingCallView` is the ringing screen while the app is open;
// `CallBar` is the "On a call · Join" strip in a chat.

import SwiftUI
import LiveKit

/// A participant's picture: coloured tree for agents, GitHub picture (or initial) for people.
struct CallPersonAvatar: View {
    let person: CallPerson
    var size: CGFloat
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme

    var body: some View {
        if person.isAgent {
            // The same identity the chat draws (#1592): a shared agent on a call is not a person.
            AgentAvatar(project: model.agent(id: person.id, handle: person.handle, name: person.name), theme: theme, size: size, still: true)
        } else {
            PersonAvatar(url: person.avatarUrl, handle: person.handle, size: size, name: person.name, id: person.id, colorIndex: person.avatarColorIndex)
        }
    }
}

struct CallScreen: View {
    @ObservedObject private var calls = CallManager.shared
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    /// Who we expected to talk to (shown before they join). Groups pass every other member + agents.
    var expected: [CallPerson] = []
    /// Share-screen confirmation: pick how to start (system picker chooses the content).
    @State private var showShareOptions = false

    /// Legacy `kind` still arrives from the API; the UI no longer treats voice and
    /// video as separate call types — every call starts audio-only and the camera
    /// can be turned on from the controls.
    private var title: String { "Call" }
    private var primary: CallTile? { calls.tiles.first }
    private var primaryPerson: CallPerson? {
        primary?.person ?? expected.first ?? calls.call?.participants.first { $0.id != model.user?.id } ?? calls.call?.startedBy
    }
    /// Group chat (people group or shared project with several expected people).
    private var isGroupCall: Bool {
        if expected.count >= 2 { return true }
        guard let id = calls.call?.chatId, let chat = model.peopleChat(id) else { return false }
        return chat.isGroup
    }
    private var groupTitle: String? {
        guard let id = calls.call?.chatId, let chat = model.peopleChat(id), chat.isGroup else { return nil }
        return chat.title.isEmpty ? "Group" : chat.title
    }
    /// Expected people who have not joined LiveKit yet (humans still ringing, agents until Phase 2).
    private var waitingExpected: [CallPerson] {
        expected.filter { e in !calls.tiles.contains { $0.person.id == e.id } }
    }
    private var useGroupLayout: Bool { calls.tiles.count + waitingExpected.count >= 2 || (isGroupCall && !waitingExpected.isEmpty && calls.tiles.count <= 1) }

    var body: some View {
        ZStack {
            theme.bg.ignoresSafeArea()
            VStack(spacing: 0) {
                ZStack {
                    TimelineView(.periodic(from: .now, by: 1)) { _ in
                        Text(headline)
                            .font(theme.font(Theme.small, weight: .semibold))
                            .foregroundStyle(theme.muted)
                    }
                    HStack {
                        Button { collapse() } label: {
                            GlassCircle {
                                Image(systemName: "chevron.down").font(.system(size: Controls.glyph, weight: .semibold)).foregroundStyle(theme.text)
                            }
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Back to chat")
                        .accessibilityHint("Keeps the call going in a small bar over the chat")
                        Spacer()
                    }
                    .padding(.horizontal, 16)
                }
                .padding(.top, 8)
                if useGroupLayout {
                    // Fill the space between header and controls so a tall group
                    // grid shrinks on short phones instead of overflowing under the
                    // Dynamic Island and clipping the hang-up row.
                    grid.frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    Spacer(minLength: 12)
                    oneToOne
                    Spacer(minLength: 12)
                }
                controls.padding(.bottom, 40)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            if let err = calls.error {
                VStack {
                    Spacer()
                    Text(err).font(theme.font(Theme.tiny)).foregroundStyle(theme.danger)
                        .multilineTextAlignment(.center).padding(.horizontal, 32).padding(.bottom, 150)
                }
            }
        }
        .statusBarHidden(false)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(title)
        // Pull down anywhere on the screen: same as the chevron.
        .simultaneousGesture(
            DragGesture(minimumDistance: 40)
                .onEnded { v in
                    if v.translation.height > 120, v.translation.height > abs(v.translation.width) * 1.5 { collapse() }
                }
        )
    }

    /// Collapse into the in-call pill and make sure the call's chat is on screen.
    private func collapse() {
        guard calls.isActive, let c = calls.call else { return }
        if let pc = model.peopleChat(c.chatId) {
            if !model.isShowing(chatId: pc.id) { model.open(peopleChat: pc) }
        } else if let chat = model.chat(c.chatId) {
            if !model.isShowing(chatId: chat.id) { model.open(chat: chat) }
        }
        calls.minimize()
    }

    private var headline: String {
        switch calls.phase {
        case .connected: return "\(title) · \(CallScreen.clock(calls.duration))"
        case .reconnecting: return "\(title) · Reconnecting…"
        default: return title
        }
    }

    private var status: String {
        switch calls.phase {
        case .connecting: return "Calling…"
        case .reconnecting: return "Reconnecting…"
        case .connected:
            if calls.tiles.isEmpty {
                if let line = ringingStatus { return line }
                return "Ringing…"
            }
            if calls.screenShareOn { return "Sharing screen" }
            return primary?.micOn == false ? "Muted" : "Connected"
        case .ringing: return "Incoming…"
        case .idle: return calls.error ?? "Call ended"
        }
    }

    /// Status while nobody has joined yet. Several callees join with " · " (Mac `reachable_line`).
    private var ringingStatus: String? {
        let reach = calls.call?.reachable ?? []
        if !reach.isEmpty {
            let lines: [String] = reach.compactMap { r in
                if r.anyLive { return nil }
                if r.canRingClosed { return "Ringing \(r.displayName)'s iPhone" }
                return "\(r.displayName)'s Arbos isn't open — they'll see a missed call"
            }
            guard !lines.isEmpty else { return nil }
            if lines.count <= 2 { return lines.joined(separator: " · ") }
            return "\(lines[0]) · \(lines[1]) · +\(lines.count - 2) more"
        }
        if calls.calleeUnreachable {
            if isGroupCall { return "Nobody's Arbos is open — they'll see a missed call" }
            let name = calls.calleeName?(calls.call?.chatId ?? "") ?? "Their"
            return "\(name)'s Arbos isn't open — they'll see a missed call"
        }
        return nil
    }

    // MARK: 1:1

    private var oneToOne: some View {
        VStack(spacing: 0) {
            ZStack(alignment: .topTrailing) {
                RoundedRectangle(cornerRadius: 28, style: .continuous)
                    .fill(theme.card)
                    .overlay(RoundedRectangle(cornerRadius: 28, style: .continuous).strokeBorder(theme.fieldBorder, lineWidth: 1)) // control-size: exception callControls
                    .overlay {
                        if let track = primary?.videoTrack, primary?.cameraOn == true {
                            SwiftUIVideoView(track, layoutMode: .fill)
                                .clipShape(RoundedRectangle(cornerRadius: 28, style: .continuous))
                        } else {
                            VStack(spacing: 16) {
                                if let p = primaryPerson {
                                    CallPersonAvatar(person: p, size: 108)
                                        .overlay(speakingRing(primary?.isSpeaking == true, size: 108))
                                } else {
                                    GlassCircle(size: 108) { Image(systemName: "person.fill").font(.system(size: 40)).foregroundStyle(theme.muted) } // control-size: exception callControls
                                }
                                Image(systemName: primary?.cameraOn == true ? "video.fill" : "waveform")
                                    .font(theme.font(22, weight: .semibold))
                                    .foregroundStyle(theme.muted)
                            }
                        }
                    }
                    .frame(width: 270, height: 352)
                if calls.cameraOn {
                    selfPreview(cornerRadius: 14)
                        .frame(width: 84, height: 112)
                        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(theme.bg, lineWidth: 2))
                        .overlay(alignment: .bottom) { AgentSeesBadge(compact: true).padding(6) }
                        .padding(10)
                }
            }
            Text(groupTitle ?? primaryPerson?.displayName ?? calls.call?.startedBy?.displayName ?? "Call")
                .font(theme.font(28, weight: .semibold))
                .foregroundStyle(theme.text)
                .padding(.top, 28)
            Text(status)
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.muted)
                .padding(.top, 6)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 32)
        }
    }

    // MARK: Group

    private var grid: some View {
        let tileCount = calls.tiles.count + waitingExpected.count + (calls.cameraOn ? 1 : 0)
        return VStack(spacing: 14) {
            GeometryReader { geo in
                let cols = 2
                let rows = max(1, (tileCount + cols - 1) / cols)
                let gap: CGFloat = 10
                let hPad: CGFloat = 20
                let tileW = max(0, geo.size.width - hPad * 2 - gap) / CGFloat(cols)
                let idealH = tileW * 4 / 3
                let fitH = max(0, geo.size.height - gap * CGFloat(rows - 1)) / CGFloat(rows)
                let tileH = max(56, min(idealH, fitH))
                LazyVGrid(columns: [GridItem(.flexible(), spacing: gap), GridItem(.flexible(), spacing: gap)], spacing: gap) {
                    ForEach(calls.tiles) { tile in
                        personTile(tile.person, videoTrack: tile.videoTrack, cameraOn: tile.cameraOn, micOn: tile.micOn, speaking: tile.isSpeaking, height: tileH)
                    }
                    ForEach(waitingExpected) { person in
                        personTile(person, videoTrack: nil, cameraOn: false, micOn: true, speaking: false, height: tileH)
                    }
                    if calls.cameraOn {
                        selfPreview(cornerRadius: 20)
                            .frame(maxWidth: .infinity)
                            .frame(height: tileH)
                            .overlay(alignment: .bottomLeading) { AgentSeesBadge(compact: false).padding(8) }
                    }
                }
                .padding(.horizontal, hPad)
                .frame(width: geo.size.width, height: geo.size.height, alignment: .center)
            }
            if let groupTitle {
                Text(groupTitle).font(theme.font(22, weight: .semibold)).foregroundStyle(theme.text)
            }
            Text(status)
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.muted)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 24)
            if calls.call?.isHostedSpeak == true, calls.call?.canSpeak == false {
                Text("Waiting for the host to invite you to speak")
                    .font(theme.font(Theme.small))
                    .foregroundStyle(theme.muted)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 24)
            } else if !calls.tiles.isEmpty {
                Text("\(calls.tiles.count + 1) in the call").font(theme.font(Theme.small)).foregroundStyle(theme.muted)
            }
        }
        .padding(.top, 12)
    }

    private var iAmHost: Bool {
        calls.call?.isHost(model.user?.id) == true
    }

    private func personTile(_ person: CallPerson, videoTrack: VideoTrack?, cameraOn: Bool, micOn: Bool, speaking: Bool, height: CGFloat) -> some View {
        let avatar = min(64, max(28, height * 0.45))
        let role = calls.call?.speakRole(of: person.id)
        let canManage = iAmHost
            && calls.call?.isHostedSpeak == true
            && person.id != model.user?.id
            && !person.isAgent
        return ZStack(alignment: .bottomLeading) {
            RoundedRectangle(cornerRadius: 20, style: .continuous).fill(theme.card)
                .overlay(RoundedRectangle(cornerRadius: 20, style: .continuous)
                    .strokeBorder(speaking ? theme.online : theme.fieldBorder, lineWidth: speaking ? 2 : 1))
            if let track = videoTrack, cameraOn {
                SwiftUIVideoView(track, layoutMode: .fill).clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
            } else {
                CallPersonAvatar(person: person, size: avatar).frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            VStack(alignment: .leading, spacing: 4) {
                if let role {
                    Text(role)
                        .font(theme.font(10, weight: .semibold))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 7).padding(.vertical, 3)
                        .background(Capsule().fill(.black.opacity(0.45)))
                }
                HStack(spacing: 5) {
                    if !micOn { Image(systemName: "mic.slash.fill").font(.system(size: 10, weight: .bold)) }
                    Text(person.displayName).font(theme.font(12, weight: .semibold)).lineLimit(1)
                }
                .foregroundStyle(.white)
                .padding(.horizontal, 8).padding(.vertical, 4)
                .background(Capsule().fill(.black.opacity(0.45)))
            }
            .padding(8)
        }
        .frame(maxWidth: .infinity)
        .frame(height: height)
        .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
        .contextMenu {
            if canManage {
                if calls.call?.isInvitedSpeaker(person.id) == true {
                    Button("Remove speak") { Task { await calls.revokeSpeaker(person.id) } }
                } else {
                    Button("Invite to speak") { Task { await calls.inviteSpeaker(person.id) } }
                }
            }
        }
    }

    /// Our camera: the live track, or a quiet placeholder until it is published.
    @ViewBuilder
    private func selfPreview(cornerRadius: CGFloat) -> some View {
        if let local = calls.localVideoTrack {
            SwiftUIVideoView(local, layoutMode: .fill, mirrorMode: .mirror)
                .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
        } else {
            RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                .fill(theme.card)
                .overlay(Image(systemName: "video.fill").font(.system(size: 18, weight: .semibold)).foregroundStyle(theme.muted))
        }
    }

    // MARK: Controls

    /// The agent this call is with (my own, not a person's): the one that may look
    /// through my camera. `nil` for calls with people.
    private var visionAgent: Project? {
        guard let c = calls.call, !c.isPeople, let p = model.project(c.projectId), !p.shared else { return nil }
        return p
    }

    private var listenOnly: Bool {
        calls.call?.isHostedSpeak == true && calls.call?.canSpeak == false
    }

    private var controls: some View {
        // Share adds a fifth control; allow a short horizontal scroll on narrow phones
        // so Mute / Camera / Share / Flip|Speaker / Hang up stay tappable.
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 18) {
                control(
                    icon: (calls.muted || listenOnly) ? "mic.slash.fill" : "mic.fill",
                    label: listenOnly ? "Listen only" : (calls.muted ? "Unmute" : "Mute"),
                    on: calls.muted || listenOnly
                ) {
                    guard !listenOnly else { return }
                    Task { await calls.toggleMute() }
                }
                control(icon: calls.cameraOn ? "video.fill" : "video.slash.fill", label: calls.cameraOn ? "Camera off" : "Camera on", on: !calls.cameraOn) {
                    Task { await calls.toggleCamera() }
                }
                control(
                    icon: calls.screenShareOn ? "rectangle.on.rectangle.slash" : "rectangle.on.rectangle",
                    label: calls.screenShareOn ? "Stop share" : "Share",
                    on: calls.screenShareOn
                ) {
                    if calls.screenShareOn {
                        Task { await calls.toggleScreenShare() }
                    } else {
                        showShareOptions = true
                    }
                }
                if calls.cameraOn, let agent = visionAgent {
                    // "Let <agent> see" (#1353): the one switch for the agent's vision_enabled,
                    // beside the camera while it is on (the Mac's call screen has the same).
                    VStack(spacing: 8) {
                        Toggle("", isOn: Binding(get: { agent.visionEnabled }, set: { model.setVision(agent.id, enabled: $0) }))
                            .labelsHidden()
                            .tint(theme.online)
                            .frame(height: 64)
                        Text("Let \(agent.name) see").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(1)
                    }
                    .accessibilityLabel("Let \(agent.name) see your camera")
                } else if calls.cameraOn {
                    control(icon: "arrow.triangle.2.circlepath.camera", label: "Flip", on: false) { Task { await calls.flipCamera() } }
                } else {
                    control(icon: calls.speakerOn ? "speaker.wave.3.fill" : "speaker.fill", label: "Speaker", on: calls.speakerOn) { calls.toggleSpeaker() }
                }
                Button { Task { await calls.hangUp() } } label: {
                    VStack(spacing: 8) {
                        Circle().fill(theme.danger).frame(width: 64, height: 64)
                            .overlay(Image(systemName: "phone.down.fill").font(.system(size: 24, weight: .semibold)).foregroundStyle(.white))
                        Text("Hang up").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                    }
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Hang up")
            }
            .padding(.horizontal, 12)
        }
        .confirmationDialog("Share screen", isPresented: $showShareOptions, titleVisibility: .visible) {
            Button("Choose what to share…") {
                Task { await calls.toggleScreenShare() }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Pick a screen or app to share with everyone on the call.")
        }
    }

    private func control(icon: String, label: String, on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 8) {
                ZStack {
                    if on { Circle().fill(theme.text).frame(width: 64, height: 64) } else { GlassCircle(size: 64) { EmptyView() } } // control-size: exception callControls
                    Image(systemName: icon).font(.system(size: 22, weight: .medium)).foregroundStyle(on ? theme.bg : theme.text)
                }
                Text(label).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }

    @ViewBuilder
    private func speakingRing(_ speaking: Bool, size: CGFloat) -> some View {
        if speaking {
            Circle().strokeBorder(theme.online, lineWidth: 3).frame(width: size + 10, height: size + 10)
        }
    }

    static func clock(_ t: TimeInterval) -> String {
        let s = Int(t)
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, (s / 60) % 60, s % 60) : String(format: "%02d:%02d", s / 60, s % 60)
    }
}

/// "<Agent> can see" on OUR self-preview while the voice worker reports that the
/// agent is sampling our camera (§11). Nothing when it is not. The compact pill (the 84 pt
/// self-preview on a 1:1 agent call) has no room for a name: it says "can see"; the
/// accessibility label still names the agent.
struct AgentSeesBadge: View {
    @ObservedObject private var calls = CallManager.shared
    @Environment(\.theme) private var theme
    var compact: Bool

    var body: some View {
        if calls.agentSeeing, (calls.cameraOn || calls.screenShareOn) {
            HStack(spacing: 4) {
                Image(systemName: "eye.fill").font(.system(size: compact ? 9 : 10, weight: .bold))
                Text(compact ? "can see" : "\(calls.seeingAgentName ?? "Agent") can see")
                    .font(theme.font(compact ? 9 : 11, weight: .semibold))
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
            }
            .foregroundStyle(.white)
            .padding(.horizontal, compact ? 6 : 8).padding(.vertical, compact ? 3 : 4)
            .background(Capsule().fill(.black.opacity(0.55)))
            .overlay(Capsule().strokeBorder(.white.opacity(0.25), lineWidth: 0.5))
            .transition(.opacity)
            .accessibilityLabel("\(calls.seeingAgentName ?? "The agent") can see your \(calls.screenShareOn && !calls.cameraOn ? "screen" : "camera")")
        }
    }
}

/// Ringing while the app is open (CallKit shows the system screen otherwise).
struct IncomingCallView: View {
    @ObservedObject private var calls = CallManager.shared
    @Environment(\.theme) private var theme

    var body: some View {
        ZStack {
            theme.bg.ignoresSafeArea()
            if let inc = calls.incoming {
                VStack(spacing: 0) {
                    Text("Incoming call")
                        .font(theme.font(Theme.small, weight: .semibold)).foregroundStyle(theme.muted)
                        .padding(.top, 14)
                    Spacer()
                    if let p = inc.from ?? inc.call.startedBy {
                        CallPersonAvatar(person: p, size: 128)
                            .overlay(Circle().strokeBorder(theme.fieldBorder, lineWidth: 1).frame(width: 150, height: 150)) // control-size: exception callControls
                        Text(p.displayName).font(theme.font(28, weight: .semibold)).foregroundStyle(theme.text).padding(.top, 28)
                        Text("@\(p.handle)").font(theme.font(Theme.body)).foregroundStyle(theme.muted).padding(.top, 4)
                    }
                    Spacer()
                    HStack(spacing: 72) {
                        VStack(spacing: 8) {
                            Button { calls.decline() } label: {
                                Circle().fill(theme.danger).frame(width: 72, height: 72)
                                    .overlay(Image(systemName: "phone.down.fill").font(.system(size: 26, weight: .semibold)).foregroundStyle(.white))
                            }
                            .buttonStyle(.plain)
                            Text("Decline").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                        }
                        VStack(spacing: 8) {
                            Button { calls.acceptFromUI() } label: {
                                Circle().fill(theme.online).frame(width: 72, height: 72)
                                    .overlay(Image(systemName: "phone.fill").font(.system(size: 26, weight: .semibold)).foregroundStyle(.white))
                            }
                            .buttonStyle(.plain)
                            Text("Accept").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                        }
                    }
                    .padding(.bottom, 56)
                }
            }
        }
    }
}

/// "On a call · Join" under a chat header while a call is active there.
struct CallBar: View {
    let call: Call
    let onJoin: () -> Void
    @Environment(\.theme) private var theme
    @ObservedObject private var calls = CallManager.shared

    var body: some View {
        Button(action: onJoin) {
            HStack(spacing: 10) {
                Image(systemName: "phone.fill").font(.system(size: 13, weight: .semibold)).foregroundStyle(theme.online)
                Text(calls.call?.callId == call.callId ? "On this call" : "On a call")
                    .font(theme.font(Theme.small, weight: .semibold)).foregroundStyle(theme.text)
                if !call.participants.isEmpty {
                    Text("· \(call.participants.count)").font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                }
                Spacer()
                Text(calls.call?.callId == call.callId ? "Return" : "Join")
                    .font(theme.font(Theme.tiny, weight: .semibold)).foregroundStyle(theme.onPrimary)
                    .padding(.horizontal, 12).frame(height: 28)
                    .background(Capsule().fill(theme.primary))
            }
            .padding(.horizontal, 14).frame(height: 44)
            .background(Capsule().fill(theme.card))
            .overlay(Capsule().strokeBorder(theme.fieldBorder, lineWidth: 1)) // control-size: exception callControls
        }
        .buttonStyle(.plain)
    }
}
