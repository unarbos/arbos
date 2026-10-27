// Calls v1: one LiveKit room per call, CallKit for the native in-call UI,
// audio session and lock-screen controls, PushKit for ringing when the app
// is closed (switches on when the relay has an APNs key), and ringing from
// `/me/pushes` `call_started` while the app is open. The relay owns the
// call record; this type owns the media and the CallKit mirror of it.
//
// Lifecycle (contract §Adjustments): start → POST …/calls (201 = we started
// it → write "<name> started a … call"), connect; accept → POST
// /calls/{id}/join, connect; hang up → disconnect, POST /calls/{id}/leave →
// `{ended, duration_s}`; the client told `ended: true` writes "Call ended ·
// N min". A `call_ended` push names the `writer_device` for relay-ended
// calls.

import Foundation
import UIKit
import AVFoundation
import AudioToolbox
import CallKit
import PushKit
import LiveKit
import ArbosFFI
import CryptoKit
import os

private let log = Logger(subsystem: "com.unarbos.arbos.ios", category: "calls")
private let audioLog = Logger(subsystem: "com.unarbos.arbos.ios", category: "call-audio")

/// How a VoIP push should satisfy CallKit when the app may already know about a call.
enum VoipPlan: Equatable {
    /// Idle: report and start ringing as usual.
    case ring
    /// Same call already ringing or active: report again (CallKit returns `callUUIDAlreadyExists`).
    case reportDuplicate
    /// A different call while one is active or ringing: report then end; leave the current call alone.
    case reportAndEnd
}

/// Pure decision for a VoIP push: always report to CallKit; only `.ring` changes call state.
func voipPlan(isActive: Bool, incomingCallId: String?, currentCallId: String?, newCallId: String) -> VoipPlan {
    if incomingCallId == newCallId || currentCallId == newCallId {
        return .reportDuplicate
    }
    if isActive || incomingCallId != nil {
        return .reportAndEnd
    }
    return .ring
}

/// How `CXAnswerCallAction` should finish: fulfill promptly, or fail without joining.
enum AnswerPlan: Equatable {
    /// No incoming call, or the microphone is already denied — fail the action.
    case fail
    /// Fulfill the action at once, then join the room asynchronously.
    case fulfillThenJoin
}

/// Pure decision for answering via CallKit: never await the network before fulfilling.
func answerPlan(hasIncoming: Bool, micDenied: Bool) -> AnswerPlan {
    if !hasIncoming || micDenied {
        return .fail
    }
    return .fulfillThenJoin
}

/// One stable CallKit UUID per relay `callId` (128-bit hash with version/variant bits set).
func callKitUUID(forCallId callId: String) -> UUID {
    var bytes = Array(SHA256.hash(data: Data(callId.utf8)).prefix(16))
    bytes[6] = (bytes[6] & 0x0F) | 0x50 // version 5
    bytes[8] = (bytes[8] & 0x3F) | 0x80 // RFC 4122 variant
    return UUID(uuid: (
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11],
        bytes[12], bytes[13], bytes[14], bytes[15]
    ))
}

/// A remote participant as the UI needs it.
struct CallTile: Identifiable, Equatable {
    var id: String
    var person: CallPerson
    var isSpeaking: Bool
    var micOn: Bool
    var cameraOn: Bool
    var videoTrack: VideoTrack?

    static func == (a: CallTile, b: CallTile) -> Bool {
        a.id == b.id && a.person == b.person && a.isSpeaking == b.isSpeaking && a.micOn == b.micOn && a.cameraOn == b.cameraOn
            && (a.videoTrack?.sid == b.videoTrack?.sid)
    }
}

@MainActor
final class CallManager: NSObject, ObservableObject {
    static let shared = CallManager()

    enum Phase: Equatable { case idle, ringing, connecting, connected, reconnecting }

    @Published private(set) var phase: Phase = .idle
    @Published private(set) var call: Call?
    /// An incoming call ringing on this device (CallKit mirrors it).
    @Published private(set) var incoming: (call: Call, from: CallPerson?)?
    @Published private(set) var tiles: [CallTile] = []
    @Published private(set) var connectedAt: Date?
    /// The last failure (the shared element draws it under the call header).
    @Published private(set) var error: UserError?
    /// The words alone, for the idle headline.
    var errorText: String? { error?.sentence }
    @Published private(set) var muted = false
    @Published private(set) var cameraOn = false
    /// Publishing a screen-share track (system picker chooses what to share on iOS 27+).
    @Published private(set) var screenShareOn = false
    @Published private(set) var speakerOn = false
    @Published private(set) var frontCamera = true
    /// Full-screen call UI wanted (the root view presents it). While a call is
    /// active and this is false the call is collapsed into `InCallOverlay`
    /// over the chat; the LiveKit/CallKit session is unaffected either way.
    @Published var isPresented = false

    /// Collapse the call screen into the pill / picture-in-picture tile.
    func minimize() {
        guard isActive else { return }
        isPresented = false
    }

    /// Bring the full call screen back.
    func restore() {
        guard isActive else { return }
        isPresented = true
    }
    /// `push` from the VoIP registration: false until the relay has an APNs key.
    @Published private(set) var voipPushEnabled: Bool?
    /// What happened with the VoIP token, for Settings → Help and the logs.
    @Published private(set) var voipStatus: String = "no token yet"
    /// Someone other than us was in the call at some point (else it was a missed call).
    private var anyoneJoined = false
    private var outgoingTimeout: Task<Void, Never>?
    /// Nobody picks up within this → hang up as a missed call.
    static let missedAfter: TimeInterval = 30
    /// Active calls per chat (for the "On a call · Join" bar), from pushes, presence and GET /chats/{id}/call.
    @Published private(set) var activeByChat: [String: Call] = [:]
    /// Member ids we asked the relay to ring on the call we started (`ring` body). Nil =
    /// whole group / DM (default). Cleared when the call tears down. The call screen uses
    /// this so a selective group start only shows the people we named as "Ringing…".
    @Published private(set) var ringIds: Set<String>?

    let room = LiveKit.Room()
    var localVideoTrack: VideoTrack? { room.localParticipant.firstCameraVideoTrack }
    var localScreenShareTrack: VideoTrack? {
        room.localParticipant.firstScreenSharePublication?.track as? VideoTrack
    }

    // Wiring from AppModel.
    var api: RelayAPI?
    var deviceId: () -> String = { "" }
    var me: () -> CallPerson? = { nil }
    var writeSystemLine: ((_ chatId: String, _ projectId: String, _ text: String, _ callId: String) -> Void)?
    var isFixture = false

    private let provider: CXProvider
    private let callController = CXCallController()
    private var cxUUID: UUID?
    private var voipRegistry: PKPushRegistry?
    private var voipToken: String?
    private var registeredVoipToken: String?
    private var ringTimeout: Task<Void, Never>?
    /// Outgoing wait: play a local ringback until someone joins (or we hang up).
    private var ringbackWanted = false
    private var ringbackPlayer: AVAudioPlayer?
    /// True between CallKit `didActivate` and `didDeactivate` — LiveKit's engine may run only then.
    private var callKitOwnsAudio = false
    /// Teardown is re-entrant from CallKit's `CXEndCallAction`; ignore nested calls.
    private var isTearingDown = false
    #if DEBUG
    private var micInjector: MicInjector?
    #endif

    private override init() {
        let config = CXProviderConfiguration()
        config.supportsVideo = true
        config.maximumCallsPerCallGroup = 1
        config.maximumCallGroups = 1
        config.supportedHandleTypes = [.generic]
        config.includesCallsInRecents = true
        if let icon = UIImage(named: "AppIcon") ?? UIImage(systemName: "phone.fill") { config.iconTemplateImageData = icon.pngData() }
        provider = CXProvider(configuration: config)
        super.init()
        provider.setDelegate(self, queue: nil)
        room.add(delegate: self)
        // CallKit owns AVAudioSession. LiveKit's automatic session config and an
        // early audio engine start fight that window and leave remote audio
        // almost inaudible (VPIO / voiceChat gain with the engine outside
        // didActivate…didDeactivate). Match LiveKit's CallKit example.
        AudioManager.shared.audioSession.isAutomaticConfigurationEnabled = false
        do {
            try AudioManager.shared.setEngineAvailability(.none)
        } catch {
            audioLog.error("engine none at launch: \(error.localizedDescription, privacy: .public)")
        }
    }

    var isActive: Bool { phase == .connecting || phase == .connected || phase == .reconnecting }
    var duration: TimeInterval { connectedAt.map { Date().timeIntervalSince($0) } ?? 0 }

    // MARK: Permissions

    static func micGranted() async -> Bool {
        switch AVAudioApplication.shared.recordPermission {
        case .granted: return true
        case .denied: return false
        default: return await AVAudioApplication.requestRecordPermission()
        }
    }

    static func cameraGranted() async -> Bool {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: return true
        case .denied, .restricted: return false
        default: return await AVCaptureDevice.requestAccess(for: .video)
        }
    }

    // MARK: Start / join / accept / leave

    /// From the chat header: start (201) or join (200) the chat's call.
    /// `ring` — optional member ids to notify (selective group call). Nil / empty = everyone.
    func start(chatId: String, projectId: String?, kind: CallKind, title: String, ring: [String]? = nil) async {
        guard !isActive else { isPresented = true; return }
        guard await CallManager.micGranted() else { error = UserError.refused("start the call", "Microphone access is off in Settings."); return }
        if kind == .video { _ = await CallManager.cameraGranted() }
        guard let api else { return }
        error = nil
        phase = .connecting
        isPresented = true
        prepareCallAudioSession(video: kind == .video)
        ringbackWanted = true
        syncRingback()
        let ringList = ring.flatMap { $0.isEmpty ? nil : $0 }
        ringIds = ringList.map { Set($0) }
        do {
            let (started, isNew) = try await api.startCall(chatId: chatId, projectId: projectId, kind: kind, ring: ringList)
            call = started
            if isNew, let who = me() {
                writeSystemLine?(chatId, started.projectId, "\(who.displayName) started a \(kind == .video ? "video" : "voice") call", started.callId)
            }
            activeByChat[chatId] = started
            reportOutgoing(title: title, video: kind == .video)
            try await connect(started, video: kind == .video)
        } catch {
            log.error("start failed: \(error.localizedDescription, privacy: .public)")
            self.error = CallManager.describe("start the call", error)
            await teardown(leave: false)
        }
    }

    /// From the "On a call · Join" bar.
    func join(_ existing: Call, title: String) async {
        guard !isActive, let api else { isPresented = true; return }
        guard await CallManager.micGranted() else { error = UserError.refused("start the call", "Microphone access is off in Settings."); return }
        error = nil
        phase = .connecting
        isPresented = true
        prepareCallAudioSession(video: existing.kind == .video)
        ringbackWanted = true
        syncRingback()
        do {
            let fresh = try await api.joinCall(existing.callId)
            call = fresh
            reportOutgoing(title: title, video: fresh.kind == .video)
            try await connect(fresh, video: fresh.kind == .video)
        } catch {
            self.error = CallManager.describe("join the call", error)
            await teardown(leave: false)
            // 410 / call_ended left the Join bar up; re-check the chat.
            await syncActiveBar(chatId: existing.chatId, projectId: existing.isPeople ? nil : existing.projectId)
        }
    }

    /// Ring: a `call_started` push while the app is open, or a VoIP push.
    /// `onCallKitReported` runs from CallKit's report callback (PushKit must finish there).
    func ring(_ incomingCall: Call, from: CallPerson?, viaVoip: Bool, onCallKitReported: (() -> Void)? = nil) {
        if viaVoip {
            switch voipPlan(
                isActive: isActive,
                incomingCallId: incoming?.call.callId,
                currentCallId: call?.callId,
                newCallId: incomingCall.callId
            ) {
            case .reportDuplicate:
                // Same call already known: report again so PushKit completes (CallKit → callUUIDAlreadyExists).
                reportVoipIncoming(incomingCall, from: from, endImmediately: false, onCallKitReported: onCallKitReported)
                return
            case .reportAndEnd:
                // Another call while busy: report then end; do not touch the active/ringing call.
                reportVoipIncoming(incomingCall, from: from, endImmediately: true, onCallKitReported: onCallKitReported)
                return
            case .ring:
                break
            }
        } else {
            guard !isActive, incoming == nil, call?.callId != incomingCall.callId else {
                onCallKitReported?()
                return
            }
        }
        incoming = (incomingCall, from)
        #if DEBUG
        if DebugHooks.autoAcceptCall {
            Task { try? await Task.sleep(nanoseconds: 1_500_000_000); await self.accept() }
        }
        #endif
        phase = .ringing
        activeByChat[incomingCall.chatId] = incomingCall
        let uuid = callKitUUID(forCallId: incomingCall.callId)
        cxUUID = uuid
        let update = Self.callKitUpdate(for: incomingCall, from: from)
        provider.reportNewIncomingCall(with: uuid, update: update) { [weak self] err in
            // CallKit rings (system ringtone + vibration, honouring the ring/silent
            // switch like a phone call). If it refuses, keep the incoming state and
            // ring in-app — clearing it dropped the only fallback and made VoIP
            // look like "they'll see a missed call" after Apple stopped delivering.
            // PushKit's completion must run here (not inside a later Task).
            if let err {
                log.error("reportNewIncomingCall: \(err.localizedDescription, privacy: .public)")
                Task { @MainActor in self?.startIncomingRingFallback() }
            } else {
                audioLog.notice("incoming: CallKit is ringing")
            }
            onCallKitReported?()
        }
        ringTimeout?.cancel()
        ringTimeout = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 60_000_000_000)
            guard let self, !Task.isCancelled, self.phase == .ringing else { return }
            self.endIncoming(reason: .unanswered)
        }
        if viaVoip { Task { await verifyStillLive(incomingCall.callId) } }
    }

    /// VoIP-only: satisfy PushKit with `reportNewIncomingCall` without changing call state.
    /// When `endImmediately`, end that reported UUID right away (busy with a different call).
    private func reportVoipIncoming(_ incomingCall: Call, from: CallPerson?, endImmediately: Bool, onCallKitReported: (() -> Void)?) {
        let uuid = callKitUUID(forCallId: incomingCall.callId)
        let update = Self.callKitUpdate(for: incomingCall, from: from)
        let provider = self.provider
        provider.reportNewIncomingCall(with: uuid, update: update) { err in
            if let err {
                log.error("reportNewIncomingCall (voip): \(err.localizedDescription, privacy: .public)")
            }
            if endImmediately {
                provider.reportCall(with: uuid, endedAt: Date(), reason: .unanswered)
            }
            onCallKitReported?()
        }
    }

    private static func callKitUpdate(for incomingCall: Call, from: CallPerson?) -> CXCallUpdate {
        let update = CXCallUpdate()
        update.remoteHandle = CXHandle(type: .generic, value: from?.displayName ?? incomingCall.startedBy?.displayName ?? "Arbos")
        update.localizedCallerName = from?.displayName ?? incomingCall.startedBy?.displayName ?? "Arbos"
        update.hasVideo = incomingCall.kind == .video
        update.supportsHolding = false
        update.supportsGrouping = false
        update.supportsUngrouping = false
        update.supportsDTMF = false
        return update
    }

    /// A VoIP-woken phone checks the call did not end in the meantime.
    private func verifyStillLive(_ callId: String) async {
        guard let api, let live = try? await api.call(callId) else { return }
        if live.ended {
            // Drop the Join bar too — endIncoming alone left "On a call · Join" up.
            noteActive(nil, chatId: live.chatId)
            endIncoming(reason: .remoteEnded)
        }
    }

    // MARK: In-app incoming ring (only when CallKit cannot report the call)

    private var incomingRingPlayer: AVAudioPlayer?
    private var incomingVibration: Task<Void, Never>?

    private func startIncomingRingFallback() {
        guard incoming != nil, incomingRingPlayer == nil else { return }
        // CallKit refused the report: we own the session and must activate it.
        prepareCallAudioSession(video: incoming?.call.kind == .video, activate: true)
        do {
            let player = try AVAudioPlayer(data: CallManager.ringtoneWAV)
            player.numberOfLoops = -1
            player.volume = 0.8
            player.prepareToPlay()
            _ = player.play()
            incomingRingPlayer = player
            audioLog.notice("incoming: in-app ring playing (CallKit refused)")
        } catch {
            audioLog.error("incoming ring: \(error.localizedDescription, privacy: .public)")
        }
        incomingVibration?.cancel()
        incomingVibration = Task { [weak self] in
            while !Task.isCancelled, self?.incoming != nil {
                AudioServicesPlaySystemSound(kSystemSoundID_Vibrate)
                try? await Task.sleep(nanoseconds: 2_000_000_000)
            }
        }
    }

    private func stopIncomingRing() {
        incomingRingPlayer?.stop()
        incomingRingPlayer = nil
        incomingVibration?.cancel()
        incomingVibration = nil
    }

    /// Incoming ring: a two-tone trill, 1 s on / 2 s off.
    private static let ringtoneWAV: Data = {
        let sampleRate = 44_100
        let cycle = sampleRate * 3
        var pcm = Data(count: cycle * 2)
        pcm.withUnsafeMutableBytes { raw in
            guard let base = raw.bindMemory(to: Int16.self).baseAddress else { return }
            let amp = 0.3 * Double(Int16.max)
            for i in 0..<sampleRate {
                let t = Double(i) / Double(sampleRate)
                let trill = sin(2 * Double.pi * 20 * t) > 0 ? 1.0 : 0.0
                let s = sin(2 * Double.pi * 880 * t) * 0.6 + sin(2 * Double.pi * 1320 * t) * 0.4
                base[i] = Int16(amp * s * trill)
            }
        }
        return CallManager.wav(pcm, sampleRate: sampleRate)
    }()

    static func wav(_ pcm: Data, sampleRate: Int) -> Data {
        var wav = Data()
        wav.reserveCapacity(44 + pcm.count)
        func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { wav.append(contentsOf: $0) } }
        func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { wav.append(contentsOf: $0) } }
        wav.append(contentsOf: [0x52, 0x49, 0x46, 0x46])
        u32(UInt32(36 + pcm.count))
        wav.append(contentsOf: [0x57, 0x41, 0x56, 0x45, 0x66, 0x6D, 0x74, 0x20])
        u32(16); u16(1); u16(1); u32(UInt32(sampleRate)); u32(UInt32(sampleRate * 2)); u16(2); u16(16)
        wav.append(contentsOf: [0x64, 0x61, 0x74, 0x61])
        u32(UInt32(pcm.count))
        wav.append(pcm)
        return wav
    }

    /// In-app Accept: answer through CallKit when we reported the call so
    /// `didActivate` runs and the LiveKit engine is allowed to open.
    func acceptFromUI() {
        if let uuid = cxUUID, incoming != nil {
            let action = CXAnswerCallAction(call: uuid)
            callController.request(CXTransaction(action: action)) { [weak self] err in
                if let err {
                    log.error("CXAnswerCallAction: \(err.localizedDescription, privacy: .public)")
                    Task { @MainActor in await self?.accept() }
                }
            }
        } else {
            Task { await accept() }
        }
    }

    /// Non-CallKit accept (in-app without a reported UUID, or CallKit request error fallback).
    func accept() async {
        let micDenied = AVAudioApplication.shared.recordPermission == .denied
        switch answerPlan(hasIncoming: incoming != nil, micDenied: micDenied) {
        case .fail:
            if incoming != nil, micDenied {
                error = UserError.refused("answer the call", "Microphone access is off in Settings.")
                endIncoming(reason: .failed)
            }
            return
        case .fulfillThenJoin:
            break
        }
        guard let inc = startAccept() else { return }
        await joinAccept(inc)
    }

    /// Sync half of answering: no awaits. Returns the incoming call to join, or nil.
    private func startAccept() -> (call: Call, from: CallPerson?)? {
        let micDenied = AVAudioApplication.shared.recordPermission == .denied
        guard answerPlan(hasIncoming: incoming != nil, micDenied: micDenied) == .fulfillThenJoin,
              let inc = incoming else {
            return nil
        }
        ringTimeout?.cancel()
        stopIncomingRing()
        incoming = nil
        phase = .connecting
        isPresented = true
        ringbackWanted = false
        stopRingback()
        return inc
    }

    /// Async half of answering: mic/camera if needed, then join and connect.
    private func joinAccept(_ inc: (call: Call, from: CallPerson?)) async {
        guard await CallManager.micGranted() else {
            error = UserError.refused("answer the call", "Microphone access is off in Settings.")
            await teardown(leave: false, reason: .failed)
            return
        }
        if inc.call.kind == .video { _ = await CallManager.cameraGranted() }
        guard let api else {
            await teardown(leave: false, reason: .failed)
            return
        }
        do {
            let fresh = try await api.joinCall(inc.call.callId)
            call = fresh
            try await connect(fresh, video: fresh.kind == .video)
        } catch {
            self.error = CallManager.describe("join the call", error)
            await teardown(leave: false)
            await syncActiveBar(chatId: inc.call.chatId, projectId: inc.call.isPeople ? nil : inc.call.projectId)
        }
    }

    func decline() {
        ringTimeout?.cancel()
        endIncoming(reason: .declinedElsewhere)
    }

    private func endIncoming(reason: CXCallEndedReason) {
        stopIncomingRing()
        if let uuid = cxUUID { provider.reportCall(with: uuid, endedAt: Date(), reason: reason) }
        cxUUID = nil
        incoming = nil
        if phase == .ringing { phase = .idle }
    }

    func hangUp() async {
        await teardown(leave: true)
    }

    /// Outgoing: start the no-answer clock; a joiner cancels it.
    private func armOutgoingTimeout() {
        outgoingTimeout?.cancel()
        outgoingTimeout = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(CallManager.missedAfter * 1_000_000_000))
            guard let self, !Task.isCancelled, self.isActive, !self.anyoneJoined else { return }
            audioLog.notice("no answer after \(Int(CallManager.missedAfter))s — ending as missed")
            self.outgoingTimeout = nil  // teardown must not cancel the task it runs in
            self.error = nil
            await self.teardown(leave: true, reason: .unanswered)
        }
    }

    /// The DM peer's reachability for the caller's status line: the relay's
    /// `reachable[]` from the 201 start when we have it, else §9 presence.
    var calleeUnreachable: Bool {
        if let c = call, !c.reachable.isEmpty {
            return c.reachable.allSatisfy { !$0.anyLive && !$0.canRingClosed }
        }
        guard let c = call, let peer = calleePresence?(c.chatId) else { return false }
        return !peer.online && (peer.lastSeenAt.map { nowMs() - $0 > 90_000 } ?? true)
    }

    /// "Ringing Robert's iPhone": nobody has Arbos open but PushKit or APNs can still ring them.
    var calleeRingingByPush: CallReachability? {
        guard let c = call, !c.reachable.isEmpty else { return nil }
        return c.reachable.first { !$0.anyLive && $0.canRingClosed }
    }

    /// Missed-call lines already written on this device (`call_missed` arrives once per device, but the alert may repeat it).
    private var missedWritten: Set<String> = []

    /// `call_missed` (push or alert): the callee's client writes the line, once per call.
    func missedCall(callId: String, chatId: String, projectId: String, from: CallPerson?) {
        guard !missedWritten.contains(callId) else { return }
        missedWritten.insert(callId)
        activeByChat[chatId] = nil
        if incoming?.call.callId == callId { endIncoming(reason: .unanswered) }
        writeSystemLine?(chatId, projectId, "Missed call from \(from?.displayName ?? "someone")", callId)
    }
    /// Set by AppModel: the DM counterpart's presence for a chat.
    var calleePresence: ((String) -> (online: Bool, lastSeenAt: Int64?)?)?
    var calleeName: ((String) -> String?)?

    /// `call_ended` from the relay (push or alert): stop everything for that call.
    func remoteEnded(callId: String, chatId: String, durationS: Int, writerDevice: String?) {
        activeByChat[chatId] = nil
        if writerDevice == deviceId(), let c = call ?? incoming?.call ?? activeByChat[chatId], c.callId == callId || call?.callId == callId {
            writeSystemLine?(chatId, c.projectId, callEndedLine(durationS: durationS), callId)
        }
        if incoming?.call.callId == callId { endIncoming(reason: .remoteEnded) }
        if call?.callId == callId { Task { await teardown(leave: false, reason: .remoteEnded) } }
    }

    /// Someone's presence or GET /chats/{id}/call says a call is on in a chat.
    func noteActive(_ c: Call?, chatId: String) {
        if let c, !c.ended { activeByChat[chatId] = c } else { activeByChat[chatId] = nil }
    }

    #if DEBUG
    /// Dev: connect to a room with a token minted outside the relay.
    func debugJoin(url: String, token: String, kind: CallKind, chatId: String, projectId: String) async {
        guard !isActive else { isPresented = true; return }
        _ = await CallManager.micGranted()
        if kind == .video { _ = await CallManager.cameraGranted() }
        let c = Call(callId: "dev-\(chatId)", chatId: chatId, projectId: projectId, kind: kind, room: "call_dev01", livekitUrl: url,
                     token: token, tokenExpiresAt: nil, startedBy: me(), startedAt: nowMs(), endedAt: nil, reason: nil, participants: [])
        call = c
        phase = .connecting
        isPresented = true
        reportOutgoing(title: "Dev call", video: kind == .video)
        do { try await connect(c, video: kind == .video) } catch { self.error = CallManager.describe("start the call", error); await teardown(leave: false) }
    }
    #endif

    // MARK: Media

    private func connect(_ c: Call, video: Bool) async throws {
        guard let token = c.token, let url = URL(string: c.livekitUrl) else {
            throw RelayError(status: 0, message: "The call has no join token")
        }
        #if DEBUG
        if let dir = DebugHooks.micDir {
            // No audio device on a headless simulator: the engine renders without one and the
            // "microphone" is whatever MicInjector feeds from `dir`.
            LiveKitSDK.setLogLevel(.debug)
            if DebugHooks.micManual {
                do { try AudioManager.shared.setManualRenderingMode(true) } catch { audioLog.error("manual rendering: \(error.localizedDescription, privacy: .public)") }
            }
            AudioManager.shared.mixer.micVolume = 0
            AudioManager.shared.mixer.appVolume = 1
            do { try AudioManager.shared.startLocalRecording() } catch { audioLog.error("startLocalRecording: \(error.localizedDescription, privacy: .public)") }
            micInjector = MicInjector(directory: dir)
        }
        #endif
        let roomOptions = RoomOptions(
            defaultCameraCaptureOptions: CameraCaptureOptions(position: frontCamera ? .front : .back, dimensions: .h720_169, fps: 30),
            defaultAudioCaptureOptions: AudioCaptureOptions(),
            adaptiveStream: true, dynacast: true
        )
        try await room.connect(url: url.absoluteString, token: token, connectOptions: ConnectOptions(autoSubscribe: true), roomOptions: roomOptions)
        phase = .connected
        connectedAt = Date()
        if room.remoteParticipants.isEmpty { armOutgoingTimeout() } else { anyoneJoined = true }
        // Late joiner: the agent's attribute mirrors its last agent_vision event.
        for p in room.remoteParticipants.values { handleAttributes(p.attributes, of: p) }
        if let uuid = cxUUID { provider.reportOutgoingCall(with: uuid, connectedAt: Date()) }
        // Hosted group calls: stay muted until the host invites you to speak.
        if c.canSpeak == false { muted = true }
        // Publish the mic only when we should be heard. Mute later uses track mute
        // (never setMicrophone(false)) so the audio session / engine stay put.
        if !muted, c.canSpeak != false {
            try? await room.localParticipant.setMicrophone(enabled: true)
        }
        #if DEBUG
        micInjector?.start()
        #endif
        if video, AVCaptureDevice.authorizationStatus(for: .video) == .authorized {
            do { try await room.localParticipant.setCamera(enabled: true); cameraOn = true } catch { cameraOn = false }
        }
        #if DEBUG
        if DebugHooks.fakeVision {
            // No camera on the simulator: pretend ours is on and the agent sees it (§11 badge).
            cameraOn = true
            agentSeeing = true
            seeingAgentName = c.agents.first?.displayName ?? "Agent"
        }
        #endif
        speakerOn = video
        // Prefer CallKit's didActivate to open the engine; if it has not run yet
        // (slow CXStartCallAction, in-app accept without CallKit, debug join),
        // take the session ourselves so the call is not silent.
        if callKitOwnsAudio {
            applySpeakerRoute()
        } else {
            enableCallAudioEngine(video: video)
        }
        refreshTiles()
    }

    private func teardown(leave: Bool, reason: CXCallEndedReason = .remoteEnded) async {
        guard !isTearingDown else { return }
        isTearingDown = true
        defer { isTearingDown = false }
        #if DEBUG
        micInjector?.stop()
        micInjector = nil
        #endif
        ringbackWanted = false
        stopRingback()
        outgoingTimeout?.cancel()
        outgoingTimeout = nil
        disableCallAudioEngine()
        callKitOwnsAudio = false
        let missed = !anyoneJoined
        let ended = call
        let wasConnected = connectedAt
        await room.disconnect()
        // Always `reportCall(ended:)` — never `request(CXEndCallAction)` from here.
        // The provider delegate queue is main (`setDelegate(..., queue: nil)`); awaiting
        // CXEndCallAction on MainActor deadlocks Hang up (fulfill waits on teardown that
        // awaits the request), so the screen stayed on "Ringing…" with a live timer.
        if let uuid = cxUUID {
            cxUUID = nil
            provider.reportCall(with: uuid, endedAt: Date(), reason: reason)
        }
        // Drop the call UI before the network leave so Hang up always dismisses, even
        // when POST /leave is slow or fails.
        call = nil
        ringIds = nil
        tiles = []
        connectedAt = nil
        cameraOn = false
        screenShareOn = false
        agentSeeing = false
        seeingAgentName = nil
        muted = false
        anyoneJoined = false
        phase = .idle
        isPresented = false
        if leave, let ended, let api, !isFixture {
            do {
                let r = try await api.leaveCall(ended.callId)
                if r.ended {
                    activeByChat[ended.chatId] = nil
                    // Nobody ever joined: the callee's client writes "Missed call from …"
                    // from its call_missed push (§8); the caller writes nothing more.
                    if !missed { writeSystemLine?(ended.chatId, ended.projectId, callEndedLine(durationS: r.durationS), ended.callId) }
                } else {
                    // Others still on, or leave answered already_ended:true (ended:false).
                    // Re-check the chat so a dead call does not leave "On a call · Join" up.
                    await syncActiveBar(chatId: ended.chatId, projectId: ended.isPeople ? nil : ended.projectId)
                }
            } catch {
                log.error("leave: \(error.localizedDescription, privacy: .public)")
                await syncActiveBar(chatId: ended.chatId, projectId: ended.isPeople ? nil : ended.projectId)
            }
        }
        if leave, isFixture, let ended {
            activeByChat[ended.chatId] = nil
            writeSystemLine?(ended.chatId, ended.projectId, callEndedLine(durationS: Int(wasConnected.map { Date().timeIntervalSince($0) } ?? 0)), ended.callId)
        }
    }

    /// Match the Join bar to GET …/call (nil / ended → hide; live → show).
    private func syncActiveBar(chatId: String, projectId: String?) async {
        guard let api else { activeByChat[chatId] = nil; return }
        if let live = try? await api.activeCall(chatId: chatId, projectId: projectId) {
            noteActive(live, chatId: chatId)
        } else {
            activeByChat[chatId] = nil
        }
    }

    func toggleMute() async {
        // Listen-only on a hosted group call: the host has not invited you yet.
        if muted, call?.canSpeak == false { return }
        muted.toggle()
        await applyLocalMicMute(muted)
        if let uuid = cxUUID { try? await callController.request(CXTransaction(action: CXSetMutedCallAction(call: uuid, muted: muted))) }
    }

    /// Mute / unmute the published mic track (keeps the track, engine, and session as they are).
    /// CallKit's mute action uses the same path. Never calls `setMicrophone(enabled: false)`.
    fileprivate func applyLocalMicMute(_ wantMuted: Bool) async {
        // `getTrackPublication(source:)` is internal to LiveKit; use the public local list.
        let pub = room.localParticipant.localAudioTracks.first { $0.source == .microphone }
            ?? room.localParticipant.localAudioTracks.first
        switch MicMutePlan.plan(wantMuted: wantMuted, hasMicPublication: pub != nil) {
        case .muteTrack:
            try? await pub?.mute()
        case .unmuteTrack:
            try? await pub?.unmute()
        case .publish:
            try? await room.localParticipant.setMicrophone(enabled: true)
        case .leaveUnpublished:
            break
        }
    }

    /// Host invites `userId` to speak (hosted group calls). Updates local call state.
    func inviteSpeaker(_ userId: String) async {
        guard let api, let c = call, c.isHostedSpeak, c.isHost(me()?.id) else { return }
        do {
            let fresh = try await api.inviteSpeaker(callId: c.callId, userId: userId)
            applySpeakState(fresh)
        } catch {
            self.error = CallManager.describe("invite the speaker", error)
        }
    }

    /// Host revokes speak permission for `userId`.
    func revokeSpeaker(_ userId: String) async {
        guard let api, let c = call, c.isHostedSpeak, c.isHost(me()?.id) else { return }
        do {
            let fresh = try await api.revokeSpeaker(callId: c.callId, userId: userId)
            applySpeakState(fresh)
        } catch {
            self.error = CallManager.describe("change the speaker", error)
        }
    }

    /// Merge speak-role fields from a fresh call reply without dropping the LiveKit session.
    private func applySpeakState(_ fresh: Call) {
        guard var current = call, current.callId == fresh.callId else { return }
        current.speakMode = fresh.speakMode
        current.invitedSpeakers = fresh.invitedSpeakers
        if let can = fresh.canSpeak { current.canSpeak = can }
        call = current
        if current.canSpeak == false, !muted {
            muted = true
            Task { await applyLocalMicMute(true) }
        }
    }

    /// LiveKit granted or revoked publish after a mid-call invite/revoke.
    func notePublishPermission(_ canPublish: Bool) {
        guard var current = call, current.isHostedSpeak else { return }
        current.canSpeak = canPublish
        call = current
        if !canPublish, !muted {
            muted = true
            Task { await applyLocalMicMute(true) }
        }
    }

    func toggleCamera() async {
        if !cameraOn, !(await CallManager.cameraGranted()) { error = UserError.refused("turn the camera on", "Camera access is off in Settings."); return }
        let turningOn = !cameraOn
        if !turningOn {
            // Off means off at once: the indicator drops now, and the camera track is
            // unpublished (not just muted) so the agent's vision sampler gets nothing.
            if !screenShareOn { agentSeeing = false }
            cameraOn = false
            objectWillChange.send()
        }
        do {
            try await room.localParticipant.setCamera(enabled: turningOn)
            if !turningOn {
                for pub in room.localParticipant.videoTracks where pub.source == .camera {
                    if let local = pub as? LocalTrackPublication { try? await room.localParticipant.unpublish(publication: local) }
                }
            }
            cameraOn = turningOn
        } catch {
            self.error = CallManager.describe("turn the camera on", error)
        }
        objectWillChange.send()
    }

    /// Start or stop screen share. Starting presents the system content picker on iOS 27+
    /// (LiveKit ScreenCaptureKit) so the user chooses the app or screen to share; dismissing
    /// the picker cancels with no track. Stopping unpublishes the share track.
    func toggleScreenShare() async {
        let turningOn = !screenShareOn
        if !turningOn {
            if !cameraOn { agentSeeing = false }
            screenShareOn = false
            objectWillChange.send()
        }
        do {
            try await room.localParticipant.setScreenShare(enabled: turningOn)
            if !turningOn {
                for pub in room.localParticipant.videoTracks where pub.source == .screenShareVideo {
                    if let local = pub as? LocalTrackPublication { try? await room.localParticipant.unpublish(publication: local) }
                }
            }
            screenShareOn = room.localParticipant.isScreenShareEnabled()
        } catch {
            screenShareOn = room.localParticipant.isScreenShareEnabled()
            // User dismissed the content picker — not an error to show on the call screen.
            if let lk = error as? LiveKitError, lk.type == .cancelled { /* no-op */ }
            else { self.error = CallManager.describe("share the screen", error) }
        }
        objectWillChange.send()
    }

    fileprivate func syncScreenShareFromRoom() {
        let on = room.localParticipant.isScreenShareEnabled()
        if screenShareOn != on {
            screenShareOn = on
            if !on, !cameraOn { agentSeeing = false }
            objectWillChange.send()
        }
    }

    // MARK: Agent camera vision (§11)

    /// §11: the voice worker says the agent is sampling OUR camera. Canonical
    /// event = reliable data message on topic `agent_vision` from the agent
    /// participant `a_<project_id>`:
    /// `{"type":"agent_vision","participant_identity":"u_<user>","seeing":bool,
    ///   "agent_name":…,"agent":"a_…","seeing_identities":[…]}`;
    /// the same JSON sits in the agent's `agent_vision` attribute for late
    /// joiners (`seeing_identities` is the full state). True only while our
    /// camera is on; camera off, `seeing:false` or the agent leaving clears it.
    @Published private(set) var agentSeeing = false
    /// Which agent reported it (name for the "<Agent> can see" badge).
    @Published private(set) var seeingAgentName: String?

    private func applyAgentVision(_ obj: Any?, from participant: Participant?) {
        guard let d = obj as? [String: Any], d["type"] as? String == "agent_vision" else { return }
        guard let me = room.localParticipant.identity?.stringValue else { return }
        let target = d["participant_identity"] as? String
        let seeingIds = (d["seeing_identities"] as? [String]) ?? []
        let seeingFlag = (d["seeing"] as? Bool) ?? ((d["seeing"] as? NSNumber)?.boolValue ?? false)
        let seeingMe: Bool
        if target == me {
            seeingMe = seeingFlag
        } else if !seeingIds.isEmpty || target != nil {
            // An event about someone else, or the attribute mirror: the full state decides.
            seeingMe = seeingIds.contains(me)
        } else {
            return
        }
        agentSeeing = seeingMe && (cameraOn || screenShareOn)
        if seeingMe {
            let identity = (d["agent"] as? String) ?? participant?.identity?.stringValue ?? ""
            seeingAgentName = (d["agent_name"] as? String)
                ?? tiles.first { $0.id == identity }?.person.displayName
                ?? call?.agents.first?.displayName
        }
    }

    fileprivate func handleData(_ data: Data, topic: String, from participant: RemoteParticipant?) {
        guard topic == "agent_vision", let obj = try? JSONSerialization.jsonObject(with: data) else { return }
        applyAgentVision(obj, from: participant)
    }

    fileprivate func handleAttributes(_ attributes: [String: String], of participant: Participant) {
        guard let raw = attributes["agent_vision"], let data = raw.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) else { return }
        applyAgentVision(obj, from: participant)
    }

    func toggleSpeaker() {
        speakerOn.toggle()
        // Automatic session config is off (CallKit); re-apply category options + port.
        configureCallAudioSession(video: call?.kind == .video, activate: false)
    }

    func flipCamera() async {
        guard let capturer = (localVideoTrack as? LocalVideoTrack)?.capturer as? CameraCapturer else { return }
        frontCamera.toggle()
        _ = try? await capturer.switchCameraPosition()
        objectWillChange.send()
    }

    private func refreshTiles() {
        if !room.remoteParticipants.isEmpty, !anyoneJoined {
            anyoneJoined = true
            outgoingTimeout?.cancel()
        }
        tiles = room.remoteParticipants.values
            .sorted { ($0.joinedAt ?? .distantPast) < ($1.joinedAt ?? .distantPast) }
            .map { p in
                let identity = p.identity?.stringValue ?? UUID().uuidString
                let person = CallPerson.parse(metadata: p.metadata) ?? CallPerson.placeholder(identity: identity, name: p.name)
                // Prefer a screen share when they are sharing; otherwise their camera.
                let screen = p.videoTracks.first { $0.source == .screenShareVideo }?.track as? VideoTrack
                let camera = p.videoTracks.first { $0.source == .camera }?.track as? VideoTrack
                let sharing = p.isScreenShareEnabled()
                let video = screen ?? camera
                let hasVideo = sharing || p.isCameraEnabled()
                return CallTile(id: identity, person: person, isSpeaking: p.isSpeaking,
                                micOn: CallManager.remoteMicOn(p), cameraOn: hasVideo, videoTrack: video)
            }
        syncScreenShareFromRoom()
        syncRingback()
    }

    /// Whether a remote participant's mic should show as on.
    ///
    /// LiveKit's `isMicrophoneEnabled()` is false when no mic publication exists
    /// yet (`nil` → muted). That flashes "Muted" / a slash on every join before
    /// the track arrives, and stays wrong if mute metadata desyncs while audio
    /// (and speaking) still flow. Prefer real audio pubs + speaking.
    private static func remoteMicOn(_ p: RemoteParticipant) -> Bool {
        if p.isSpeaking { return true }
        let audio = p.audioTracks
        guard !audio.isEmpty else { return true }
        if let mic = audio.first(where: { $0.source == .microphone }) {
            return !mic.isMuted
        }
        return audio.contains { !$0.isMuted }
    }

    // MARK: Outgoing ringback

    /// Play while we wait for someone else ("Calling…" / "Ringing…"); stop when they join.
    private func syncRingback(force: Bool = false) {
        let waiting = ringbackWanted && tiles.isEmpty && (phase == .connecting || phase == .connected || phase == .reconnecting)
        if waiting { force ? startRingback(force: true) : resumeRingbackIfNeeded() } else { stopRingback() }
    }

    /// Category/mode for call audio. CallKit activates the session; we only
    /// `setActive` ourselves when CallKit is not in the picture (in-app ring
    /// fallback). Do not open LiveKit's engine here — that is `enableCallAudioEngine`.
    private func prepareCallAudioSession(video: Bool, activate: Bool = false) {
        configureCallAudioSession(video: video, activate: activate && !callKitOwnsAudio)
        observeAudioSession()
    }

    private func configureCallAudioSession(video: Bool, activate: Bool) {
        let session = AVAudioSession.sharedInstance()
        let mode: AVAudioSession.Mode = video ? .videoChat : .voiceChat
        var options: AVAudioSession.CategoryOptions = [.allowBluetooth, .allowBluetoothA2DP]
        if speakerOn || video { options.insert(.defaultToSpeaker) }
        do {
            try session.setCategory(.playAndRecord, mode: mode, options: options)
            // CallKit activates the session; calling setActive again fights it and
            // is a known source of near-silent remote audio with Voice Processing.
            if activate { try session.setActive(true) }
            applySpeakerRoute()
            audioLog.notice("session ready category=\(session.category.rawValue, privacy: .public) mode=\(session.mode.rawValue, privacy: .public) activate=\(activate) route=\(session.currentRoute.outputs.map(\.portType.rawValue).joined(separator: ","), privacy: .public)")
        } catch {
            audioLog.error("session setup: \(error.localizedDescription, privacy: .public)")
        }
    }

    private func applySpeakerRoute() {
        let session = AVAudioSession.sharedInstance()
        do {
            try session.overrideOutputAudioPort(speakerOn ? .speaker : .none)
        } catch {
            audioLog.error("speaker route: \(error.localizedDescription, privacy: .public)")
        }
    }

    /// Open LiveKit's audio engine after the session is call-ready (CallKit
    /// didActivate, or a non-CallKit fallback).
    private func enableCallAudioEngine(video: Bool) {
        configureCallAudioSession(video: video, activate: !callKitOwnsAudio)
        do {
            try AudioManager.shared.setEngineAvailability(.default)
            audioLog.notice("engine default (call audio live)")
        } catch {
            audioLog.error("engine default: \(error.localizedDescription, privacy: .public)")
        }
    }

    private func disableCallAudioEngine() {
        do {
            try AudioManager.shared.setEngineAvailability(.none)
            audioLog.notice("engine none")
        } catch {
            audioLog.error("engine none: \(error.localizedDescription, privacy: .public)")
        }
    }

    private var audioObservers: [NSObjectProtocol] = []

    /// A category change or interruption (LiveKit/CallKit taking the session)
    /// silently stops an AVAudioPlayer; restart the ringback while it is wanted.
    private func observeAudioSession() {
        guard audioObservers.isEmpty else { return }
        let center = NotificationCenter.default
        audioObservers.append(center.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { [weak self] n in
            let type = (n.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt).flatMap(AVAudioSession.InterruptionType.init)
            Task { @MainActor in
                audioLog.notice("interruption \(type == .began ? "began" : "ended", privacy: .public)")
                if type == .ended { self?.resumeRingbackIfNeeded() }
            }
        })
        audioObservers.append(center.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in
                let s = AVAudioSession.sharedInstance()
                audioLog.notice("route → \(s.currentRoute.outputs.map(\.portType.rawValue).joined(separator: ","), privacy: .public) category=\(s.category.rawValue, privacy: .public)")
                // A route change does not kill the player; only pick it up if it fell silent.
                try? await Task.sleep(nanoseconds: 300_000_000)
                self?.resumeRingbackIfNeeded()
            }
        })
    }

    /// The player fell silent (interruption, session handed around): play again, same instance.
    private func resumeRingbackIfNeeded() {
        guard ringbackWanted, tiles.isEmpty, isActive else { return }
        if let p = ringbackPlayer {
            if !p.isPlaying {
                let ok = p.play()
                audioLog.notice("ringback resume \(ok ? "ok" : "refused", privacy: .public)")
                if !ok { retryRingback() }
            }
        } else {
            startRingback()
        }
    }

    private var ringbackRetry: Task<Void, Never>?

    /// `play()` says no while WebRTC/CallKit are still configuring the session:
    /// keep trying every half second (re-activating the session first) until
    /// it takes or the wait is over — one loop, not one per notification.
    private func retryRingback() {
        guard ringbackRetry == nil else { return }
        ringbackRetry = Task { [weak self] in
            defer { Task { @MainActor in self?.ringbackRetry = nil } }
            for attempt in 1...40 {
                try? await Task.sleep(nanoseconds: 500_000_000)
                guard let self, !Task.isCancelled, self.ringbackWanted, self.tiles.isEmpty, self.isActive, let p = self.ringbackPlayer else { return }
                if p.isPlaying { return }
                let session = AVAudioSession.sharedInstance()
                // Do not setActive here: CallKit owns activation; fighting it quiets the call.
                let ok = p.play()
                audioLog.notice("ringback retry \(attempt) \(ok ? "ok" : "refused", privacy: .public) active-others=\(session.isOtherAudioPlaying) route=\(session.currentRoute.outputs.map(\.portType.rawValue).joined(separator: ","), privacy: .public)")
                if ok { return }
            }
        }
    }

    private func startRingback(force: Bool = false) {
        if let ringbackPlayer, ringbackPlayer.isPlaying, !force { return }
        if let p = ringbackPlayer, !force { resumeRingbackIfNeeded(); _ = p; return }
        ringbackPlayer?.stop()
        ringbackRetry?.cancel()
        ringbackRetry = nil
        let session = AVAudioSession.sharedInstance()
        if session.category != .playAndRecord { prepareCallAudioSession(video: call?.kind == .video) }
        do {
            let player = try AVAudioPlayer(data: CallManager.ringbackWAV)
            player.numberOfLoops = -1
            player.volume = 0.55
            player.prepareToPlay()
            let ok = player.play()
            ringbackPlayer = player
            audioLog.notice("ringback \(ok ? "playing" : "did not start", privacy: .public) category=\(session.category.rawValue, privacy: .public) route=\(session.currentRoute.outputs.map(\.portType.rawValue).joined(separator: ","), privacy: .public)")
            if !ok { retryRingback() }
        } catch {
            audioLog.error("ringback: \(error.localizedDescription, privacy: .public)")
            ringbackPlayer = nil
        }
    }

    private func stopRingback() {
        if ringbackPlayer != nil { audioLog.notice("ringback stopped") }
        ringbackRetry?.cancel()
        ringbackRetry = nil
        ringbackPlayer?.stop()
        ringbackPlayer = nil
    }

    /// Dual-tone ringback (≈ North American): 440 + 480 Hz, 2 s on / 4 s off.
    private static let ringbackWAV: Data = {
        let sampleRate = 44_100
        let onSamples = sampleRate * 2
        let cycleSamples = sampleRate * 6
        var pcm = Data(count: cycleSamples * 2)
        pcm.withUnsafeMutableBytes { raw in
            guard let base = raw.bindMemory(to: Int16.self).baseAddress else { return }
            let amp = 0.28 * Double(Int16.max)
            for i in 0..<onSamples {
                let t = Double(i) / Double(sampleRate)
                let sample = (sin(2 * Double.pi * 440 * t) + sin(2 * Double.pi * 480 * t)) * 0.5
                base[i] = Int16(amp * sample)
            }
            // Remaining samples stay 0 (silence for the 4 s off gap).
        }
        var wav = Data()
        wav.reserveCapacity(44 + pcm.count)
        func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { wav.append(contentsOf: $0) } }
        func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { wav.append(contentsOf: $0) } }
        wav.append(contentsOf: [0x52, 0x49, 0x46, 0x46]) // RIFF
        u32(UInt32(36 + pcm.count))
        wav.append(contentsOf: [0x57, 0x41, 0x56, 0x45, 0x66, 0x6D, 0x74, 0x20]) // WAVEfmt
        u32(16) // PCM fmt chunk size
        u16(1) // PCM
        u16(1) // mono
        u32(UInt32(sampleRate))
        u32(UInt32(sampleRate * 2)) // byte rate
        u16(2) // block align
        u16(16) // bits
        wav.append(contentsOf: [0x64, 0x61, 0x74, 0x61]) // data
        u32(UInt32(pcm.count))
        wav.append(pcm)
        return wav
    }()

    // MARK: CallKit mirror

    private func reportOutgoing(title: String, video: Bool) {
        let uuid = call.map { callKitUUID(forCallId: $0.callId) } ?? UUID()
        cxUUID = uuid
        let handle = CXHandle(type: .generic, value: title)
        let action = CXStartCallAction(call: uuid, handle: handle)
        action.isVideo = video
        callController.request(CXTransaction(action: action)) { err in
            if let err { log.error("CXStartCallAction: \(err.localizedDescription, privacy: .public)") }
        }
        let update = CXCallUpdate()
        update.remoteHandle = handle
        update.localizedCallerName = title
        update.hasVideo = video
        provider.reportCall(with: uuid, updated: update)
    }

    // MARK: PushKit (VoIP)

    func prepareVoip() {
        guard voipRegistry == nil else { return }
        let registry = PKPushRegistry(queue: .main)
        registry.delegate = self
        registry.desiredPushTypes = [.voIP]
        voipRegistry = registry
    }

    func sendVoipTokenIfNeeded(force: Bool = false) async {
        guard !isFixture else { return }
        guard let token = voipToken else { voipStatus = "no token yet"; return }
        guard let api, !deviceId().isEmpty, deviceId() != "ios:unpaired" else { voipStatus = "token ready · not signed in"; return }
        if !force, token == registeredVoipToken { return }
        registeredVoipToken = token
        do {
            let enabled = try await api.registerVoip(deviceId: deviceId(), token: token, environment: PushManager.environment,
                                                     bundleId: Bundle.main.bundleIdentifier ?? "com.unarbos.arbos.ios")
            voipPushEnabled = enabled
            voipStatus = enabled ? "registered" : "registered · relay push key pending"
            log.notice("voip token registered for device \(self.deviceId(), privacy: .public) env=\(PushManager.environment, privacy: .public) push=\(enabled)")
        } catch {
            registeredVoipToken = nil
            voipStatus = "registration failed: \(CallManager.describe(error))"
            log.error("voip register failed: \(error.localizedDescription, privacy: .public)")
        }
    }

    func forgetVoipRegistration() { registeredVoipToken = nil }

    /// The call's failure as the shared `UserError`: the relay's call states keep their own
    /// sentences as refusals; everything else goes through the classifier.
    static func describe(_ verb: String, _ error: Error) -> UserError {
        if let e = error as? RelayError {
            let said: String? = switch (e.status, e.type ?? "") {
            case (503, _), (_, "calls_off"): "Calls are not switched on for this relay yet."
            case (410, _), (_, "call_ended"): "That call has ended."
            case (409, _), (_, "call_full"): "The call is full."
            case (404, _), (405, _): "Calls are not on this relay yet."
            case (403, _): "You are not a member of this chat."
            default: nil
            }
            if let said { return UserError.refused(verb, said) }
        }
        return UserError.classify(verb, error)
    }

    /// The sentence alone (older callers).
    static func describe(_ error: Error) -> String {
        describe("start the call", error).sentence
    }

    /// An ARBOS_CALL alert (tap or foreground): `event` started → ring, ended → stop.
    func handleCallAlert(_ info: [AnyHashable: Any]) {
        var dict: [String: Any] = [:]
        for (k, v) in info { if let key = k as? String { dict[key] = v } }
        guard JSONSerialization.isValidJSONObject(dict),
              let data = try? JSONSerialization.data(withJSONObject: dict),
              let alert = try? JSONDecoder().decode(CallAlertDto.self, from: data) else { return }
        let c = Call(dto: alert.call)
        switch alert.event {
        case "missed":
            missedCall(
                callId: c.callId,
                chatId: c.chatId,
                projectId: c.projectId.isEmpty ? peopleSlot : c.projectId,
                from: alert.sender.map(CallPerson.init(dto:)) ?? c.startedBy
            )
        case "ended":
            remoteEnded(callId: c.callId, chatId: c.chatId, durationS: 0, writerDevice: nil)
        default:
            ring(c, from: alert.sender.map(CallPerson.init(dto:)) ?? c.startedBy, viaVoip: false)
        }
    }

    /// Notification Decline on an `ARBOS_CALL` alert (no Accept).
    func handleCallDecline(_ info: [AnyHashable: Any]) {
        var dict: [String: Any] = [:]
        for (k, v) in info { if let key = k as? String { dict[key] = v } }
        let callId: String?
        if JSONSerialization.isValidJSONObject(dict),
           let data = try? JSONSerialization.data(withJSONObject: dict),
           let alert = try? JSONDecoder().decode(CallAlertDto.self, from: data) {
            callId = alert.call.call_id
        } else {
            callId = nil
        }
        if let callId, incoming?.call.callId == callId {
            decline()
        } else if incoming != nil, callId == nil {
            decline()
        }
    }
}

// MARK: - LiveKit

extension CallManager: RoomDelegate {
    nonisolated func room(_ room: LiveKit.Room, didUpdateConnectionState connectionState: ConnectionState, from oldConnectionState: ConnectionState) {
        Task { @MainActor in
            switch connectionState {
            case .reconnecting: if self.phase == .connected { self.phase = .reconnecting }
            case .connected: if self.phase == .reconnecting { self.phase = .connected }
            case .disconnected:
                // Only a drop from a live session counts; the SDK also reports
                // .disconnected while a connect is still in flight.
                if (oldConnectionState == .connected || oldConnectionState == .reconnecting),
                   self.phase == .connected || self.phase == .reconnecting {
                    self.error = self.error ?? UserError.of("stay on the call", .offline(deviceOffline: false), "Connection lost.")
                    await self.teardown(leave: true, reason: .failed)
                }
            default: break
            }
            self.refreshTiles()
        }
    }

    nonisolated func room(_ room: LiveKit.Room, participantDidConnect participant: RemoteParticipant) {
        Task { @MainActor in
            self.refreshTiles()
            self.handleAttributes(participant.attributes, of: participant)
        }
    }

    nonisolated func room(_ room: LiveKit.Room, participantDidDisconnect participant: RemoteParticipant) {
        Task { @MainActor in
            self.refreshTiles()
            // The agent that was watching left: nothing is seeing us now.
            if participant.identity?.stringValue.hasPrefix("a_") == true || self.tiles.allSatisfy({ !$0.person.isAgent }) {
                self.agentSeeing = false
            }
        }
    }

    nonisolated func room(_ room: LiveKit.Room, participant: RemoteParticipant?, didReceiveData data: Data, forTopic topic: String, encryptionType: EncryptionType) {
        Task { @MainActor in self.handleData(data, topic: topic, from: participant) }
    }

    nonisolated func room(_ room: LiveKit.Room, participant: Participant, didUpdatePermissions permissions: ParticipantPermissions) {
        Task { @MainActor in
            guard participant is LocalParticipant else { return }
            self.notePublishPermission(permissions.canPublish)
        }
    }

    nonisolated func room(_ room: LiveKit.Room, participant: Participant, didUpdateAttributes attributes: [String: String]) {
        Task { @MainActor in self.handleAttributes(attributes, of: participant) }
    }

    nonisolated func room(_ room: LiveKit.Room, participant: RemoteParticipant, didSubscribeTrack publication: RemoteTrackPublication) {
        Task { @MainActor in self.refreshTiles() }
    }

    nonisolated func room(_ room: LiveKit.Room, participant: RemoteParticipant, didUnsubscribeTrack publication: RemoteTrackPublication) {
        Task { @MainActor in self.refreshTiles() }
    }

    nonisolated func room(_ room: LiveKit.Room, participant: LocalParticipant, didPublishTrack publication: LocalTrackPublication) {
        Task { @MainActor in self.syncScreenShareFromRoom() }
    }

    nonisolated func room(_ room: LiveKit.Room, participant: LocalParticipant, didUnpublishTrack publication: LocalTrackPublication) {
        Task { @MainActor in self.syncScreenShareFromRoom() }
    }

    nonisolated func room(_ room: LiveKit.Room, participant: Participant, didUpdateIsSpeaking isSpeaking: Bool) {
        Task { @MainActor in self.refreshTiles() }
    }

    nonisolated func room(_ room: LiveKit.Room, participant: Participant, didUpdateMetadata metadata: String?) {
        Task { @MainActor in self.refreshTiles() }
    }

    nonisolated func room(_ room: LiveKit.Room, participant: Participant, trackPublication: TrackPublication, didUpdateIsMuted isMuted: Bool) {
        Task { @MainActor in self.refreshTiles() }
    }
}

// MARK: - CallKit

extension CallManager: CXProviderDelegate {
    nonisolated func providerDidReset(_ provider: CXProvider) {
        // CallKit lost its state (its daemon restarted; routine on the simulator). The LiveKit
        // call is fine: forget the CallKit handle and keep talking rather than hang up on
        // everyone. The system call UI simply stops tracking this call.
        Task { @MainActor in
            log.notice("CallKit provider reset; keeping the call without a CallKit handle")
            self.cxUUID = nil
        }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXStartCallAction) {
        action.fulfill()
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXAnswerCallAction) {
        Task { @MainActor in
            // CallKit expects a prompt fulfill/fail: it will not activate the audio
            // session until then, and times out a slow join (lock-screen "Connecting").
            let micDenied = AVAudioApplication.shared.recordPermission == .denied
            switch answerPlan(hasIncoming: self.incoming != nil, micDenied: micDenied) {
            case .fail:
                if self.incoming != nil {
                    self.error = UserError.refused("start the call", "Microphone access is off in Settings.")
                    self.endIncoming(reason: .failed)
                }
                action.fail()
            case .fulfillThenJoin:
                guard let inc = self.startAccept() else {
                    action.fail()
                    return
                }
                action.fulfill()
                Task { await self.joinAccept(inc) }
            }
        }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXEndCallAction) {
        Task { @MainActor in
            // Fulfill before awaiting teardown so CallKit is never blocked on MainActor work.
            if self.incoming != nil {
                self.ringTimeout?.cancel()
                self.incoming = nil
                self.phase = .idle
                self.cxUUID = nil
                action.fulfill()
                return
            }
            let leaveRelay = self.isActive
            self.cxUUID = nil
            action.fulfill()
            if leaveRelay {
                // Ended from the system UI: leave the relay call too.
                await self.teardown(leave: true)
            }
        }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXSetMutedCallAction) {
        Task { @MainActor in
            // CallKit often fires CXSetMutedCallAction(true) during CXStartCallAction
            // / audio-session setup on recent iOS (and can flap for a moment after).
            // Applying that publishes us as muted to everyone — remotes see "Muted"
            // while our capture may still be live. Only mirror mute once connected,
            // and ignore a mute=true in the first 2s unless we already muted ourselves.
            guard self.phase == .connected || self.phase == .reconnecting else {
                action.fulfill()
                return
            }
            if action.isMuted, !self.muted,
               let at = self.connectedAt, Date().timeIntervalSince(at) < 2 {
                action.fulfill()
                return
            }
            if self.muted != action.isMuted {
                self.muted = action.isMuted
                await self.applyLocalMicMute(self.muted)
            }
            action.fulfill()
        }
    }

    nonisolated func provider(_ provider: CXProvider, didActivate audioSession: AVAudioSession) {
        audioLog.notice("audio session activated by CallKit category=\(audioSession.category.rawValue, privacy: .public)")
        // CallKit has the session: configure it and allow LiveKit's engine to run.
        // Do not setActive — CallKit already did.
        Task { @MainActor in
            self.callKitOwnsAudio = true
            let video = self.call?.kind == .video || self.incoming?.call.kind == .video
            self.enableCallAudioEngine(video: video)
            self.resumeRingbackIfNeeded()
        }
    }

    nonisolated func provider(_ provider: CXProvider, didDeactivate audioSession: AVAudioSession) {
        log.notice("audio session deactivated by CallKit")
        Task { @MainActor in
            self.callKitOwnsAudio = false
            self.disableCallAudioEngine()
        }
    }
}

// MARK: - PushKit

extension CallManager: PKPushRegistryDelegate {
    nonisolated func pushRegistry(_ registry: PKPushRegistry, didUpdate pushCredentials: PKPushCredentials, for type: PKPushType) {
        let hex = pushCredentials.token.map { String(format: "%02x", $0) }.joined()
        log.notice("voip token received (\(hex.count) hex chars)")
        Task { @MainActor in
            self.voipToken = hex
            await self.sendVoipTokenIfNeeded()
        }
    }

    nonisolated func pushRegistry(_ registry: PKPushRegistry, didInvalidatePushTokenFor type: PKPushType) {
        Task { @MainActor in self.voipToken = nil; self.registeredVoipToken = nil }
    }

    /// Payload `{call_id, chat_id, project_id, kind, room, livekit_url, caller: sender{}, started_at}`.
    /// CallKit must be told before the PushKit completion runs; the join token comes from
    /// POST /calls/{id}/join on accept. `PKPushRegistry(queue: .main)` delivers on the main
    /// queue — do not wrap this in `Task` or iOS stops VoIP delivery (WhatsApp-style ring dies).
    nonisolated func pushRegistry(_ registry: PKPushRegistry, didReceiveIncomingPushWith payload: PKPushPayload, for type: PKPushType, completion: @escaping () -> Void) {
        let dict = payload.dictionaryPayload
        var d: [String: Any] = [:]
        for (k, v) in dict { if let key = k as? String { d[key] = v } }
        let ring: CallRingPayloadDto? = {
            guard JSONSerialization.isValidJSONObject(d),
                  let data = try? JSONSerialization.data(withJSONObject: d) else { return nil }
            return try? JSONDecoder().decode(CallRingPayloadDto.self, from: data)
        }()
        let caller = ring.map { CallPerson(dto: $0.caller) }
        let parsed: Call? = ring.map { r in
            Call(
                callId: r.call_id,
                chatId: r.chat_id,
                projectId: r.project_id.isEmpty ? peopleSlot : r.project_id,
                kind: CallKind(rawValue: r.kind) ?? .voice,
                room: r.room,
                livekitUrl: r.livekit_url ?? "",
                startedBy: CallPerson(dto: r.caller),
                startedAt: r.started_at
            )
        }

        let report: @MainActor () -> Void = {
            if let parsed {
                self.ring(parsed, from: caller, viaVoip: true, onCallKitReported: completion)
            } else {
                // Report something anyway: iOS terminates apps that take a VoIP push and report no call.
                let uuid = UUID()
                let update = CXCallUpdate()
                update.remoteHandle = CXHandle(type: .generic, value: "Arbos")
                self.provider.reportNewIncomingCall(with: uuid, update: update) { _ in
                    self.provider.reportCall(with: uuid, endedAt: Date(), reason: .failed)
                    completion()
                }
            }
        }
        if Thread.isMainThread {
            MainActor.assumeIsolated(report)
        } else {
            DispatchQueue.main.async { MainActor.assumeIsolated(report) }
        }
    }
}

#if DEBUG
/// Simulator-only microphone: WAV files that appear in a directory are mixed into the local
/// audio track (`AudioManager.shared.mixer.capture(appAudio:)`), in name order, once each.
/// Lets a headless simulator take part in a real call for proof recordings.
@MainActor
final class MicInjector {
    private let directory: URL
    private var timer: Timer?
    private var played: Set<String> = []

    init(directory: String) {
        self.directory = URL(fileURLWithPath: directory, isDirectory: true)
    }

    func start() {
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        // Files already there are a backlog from a previous run; skip them.
        played = Set(pending())
        timer = Timer.scheduledTimer(withTimeInterval: 0.2, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.tick() }
        }
        audioLog.notice("mic injector watching \(self.directory.path, privacy: .public)")
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    private func pending() -> [String] {
        ((try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? [])
            .filter { $0.hasSuffix(".wav") }
            .sorted()
    }

    private func tick() {
        for name in pending() where !played.contains(name) {
            played.insert(name)
            let url = directory.appendingPathComponent(name)
            do {
                let file = try AVAudioFile(forReading: url)
                guard let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(file.length)) else { continue }
                try file.read(into: buffer)
                AudioManager.shared.mixer.capture(appAudio: buffer)
                audioLog.notice("mic injector: playing \(name, privacy: .public) (\(Double(file.length) / file.processingFormat.sampleRate, format: .fixed(precision: 2)) s)")
            } catch {
                audioLog.error("mic injector: \(name, privacy: .public): \(error.localizedDescription, privacy: .public)")
            }
        }
    }
}
#endif
