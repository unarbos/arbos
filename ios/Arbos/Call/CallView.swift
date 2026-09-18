import PhotosUI
import SwiftUI

/// Builds the call from the environment so a project chat can present it
/// full-screen without holding the model itself.
struct CallScreen: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var chat: ChatStore
    @EnvironmentObject private var link: VoiceLink

    var body: some View {
        CallView(settings: settings, chat: chat, link: link)
            .environmentObject(settings)
            .environmentObject(chat)
            .environmentObject(link)
    }
}

/// The call, in two states of one screen (Jacob's reference: ChatGPT's
/// voice mode, 2026-09-15). **Voice first**: a nearly empty screen, one
/// soft orb in the middle that moves with the live voice, a menu top left
/// and settings top right, nothing else. **Pulled down** (swipe down):
/// the same screen with a composer row at the bottom — `+`, a field with
/// a small level gauge, mute, close — so Jacob can type without leaving
/// the call. Measured from the reference: orb 195 pt a little under the
/// centre, 40 pt round controls 20 pt in, a 43 pt bottom row 36 pt in.
struct CallView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var chat: ChatStore
    @Environment(\.dismiss) private var dismiss
    @StateObject private var model: CallViewModel
    @State private var showSettings = false
    @State private var pulledDown = false
    @State private var draft = ""
    @State private var attachments: [PendingAttachment] = []
    @State private var photoItems: [PhotosPickerItem] = []
    @State private var showPhotos = false
    @State private var showFiles = false
    @FocusState private var typing: Bool

    init(settings: AppSettings, chat: ChatStore, link: VoiceLink) {
        _model = StateObject(wrappedValue: CallViewModel(settings: settings, chat: chat, link: link))
    }

    var body: some View {
        ZStack {
            ArbosTheme.bg.ignoresSafeArea()
            // The orb sits a little under the centre (the reference: centre
            // + 14 pt) and stays put when the composer comes up.
            VStack(spacing: 28) {
                VoiceOrb(level: model.level, phase: model.phase)
                    .frame(width: 200, height: 200)
                    .onTapGesture(perform: tapOrb)
                    // The screen stays wordless on purpose, and the orb's
                    // colour carries the state. Colour carries it to one kind
                    // of person only: read aloud, this screen said nothing at
                    // all about whether it was listening, thinking or
                    // speaking. The phase goes to VoiceOver, where it costs
                    // the design nothing.
                    .accessibilityElement()
                    .accessibilityLabel("Call")
                    .accessibilityValue(model.phase.label)
                    .accessibilityHint(model.phase == .idle ? "Tap to call" : "")
                    .accessibilityAddTraits(.isButton)
                // Who is on the line: the gateway's own word once the call
                // is up (`session.ready.project`), the chat's face before.
                // With the keyboard up the transcript line rises into this
                // spot (the orb layer ignores the keyboard on purpose), so the
                // name steps aside rather than print over the words.
                HStack(spacing: 8) {
                    ProjectGlyph(identity: onTheLine, size: 22)
                    Text(onTheLine.label ?? projectName)
                        .font(ArbosTheme.bodySemibold)
                        .foregroundStyle(ArbosTheme.textMuted)
                        .lineLimit(1)
                }
                .padding(.top, -8)
                .opacity(typing ? 0 : 1)
                .animation(.easeOut(duration: 0.15), value: typing)
                if let hint {
                    Text(hint)
                        .font(ArbosTheme.callout)
                        .foregroundStyle(ArbosTheme.textFaint)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 40)
                }
            }
            .offset(y: 14)
            .ignoresSafeArea(.keyboard)
            VStack(spacing: 0) {
                topBar
                Spacer(minLength: 0)
                if pulledDown {
                    pulledDownRow
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            pickers
        }
        .contentShape(Rectangle())
        .gesture(pullGesture)
        .preferredColorScheme(.dark)
        .sheet(isPresented: $showSettings, onDismiss: model.refreshIdle) {
            SettingsView().environmentObject(settings)
        }
        .onChange(of: settings.openAIKey) { _, _ in model.refreshIdle() }
        .onChange(of: settings.selfHostedURL) { _, _ in model.refreshIdle() }
        .onChange(of: settings.voiceToken) { _, _ in model.refreshIdle() }
        .onChange(of: settings.provider) { _, _ in model.refreshIdle() }
        .task { await chat.connect() }
        #if DEBUG
        .task { await previewIfAsked() }
        #endif
    }

    // MARK: - Top

    /// Menu top left (back to the chat, speaker, hang up), settings top
    /// right. 40 pt discs, 20 pt in, as the reference.
    private var topBar: some View {
        HStack {
            Menu {
                Button {
                    dismiss()
                } label: {
                    Label("Back to chat", systemImage: "chevron.down")
                }
                if model.phase.inCall {
                    Button(action: model.toggleSpeaker) {
                        Label(model.outputRoute == "speaker" ? "Use headset" : "Use speaker", systemImage: "speaker.wave.2")
                    }
                    Button(role: .destructive) {
                        model.endCall()
                        dismiss()
                    } label: {
                        Label("Hang up", systemImage: "phone.down")
                    }
                }
            } label: {
                CallDisc(symbol: "line.3.horizontal") {}
                    .allowsHitTesting(false)
            }
            Spacer()
            CallDisc(symbol: "slider.horizontal.3") { showSettings = true }
        }
        .padding(.horizontal, ArbosTheme.gutter)
        .padding(.top, 2)
    }

    // MARK: - Pulled down

    /// The reference's bottom row: the field (`+`, words, a level gauge),
    /// mute, close. 43 pt tall, 36 pt from the edges, 10 pt gaps.
    private var pulledDownRow: some View {
        VStack(spacing: 14) {
            if let line = lastLine {
                Text(line.text)
                    .font(ArbosTheme.body)
                    .lineSpacing(ArbosTheme.lineSpacing)
                    .foregroundStyle(line.speaker == .arbos ? ArbosTheme.text : ArbosTheme.textFaint)
                    .multilineTextAlignment(.center)
                    .lineLimit(3)
                    .truncationMode(.head)
                    .padding(.horizontal, 36)
            }
            if !attachments.isEmpty {
                AttachmentChips(attachments: $attachments)
            }
            HStack(spacing: 10) {
                HStack(spacing: 10) {
                    // The same `+` as the chat's: a photo or a file rides
                    // with the typed words to the kernel.
                    Menu {
                        Button { showPhotos = true } label: { Label("Photo Library", systemImage: "photo.on.rectangle") }
                        Button { showFiles = true } label: { Label("Files", systemImage: "folder") }
                    } label: {
                        Image(systemName: "plus")
                            .font(.system(size: 20, weight: .regular))
                            .foregroundStyle(attachments.isEmpty ? ArbosTheme.textMuted : ArbosTheme.accent)
                            .frame(width: 24, height: 24)
                    }
                    TextField("Type to \(projectName)", text: $draft)
                        .font(ArbosTheme.body)
                        .foregroundStyle(ArbosTheme.text)
                        .tint(ArbosTheme.accent)
                        .focused($typing)
                        .submitLabel(.send)
                        .onSubmit(sendTyped)
                    if draft.isEmpty && attachments.isEmpty {
                        LevelGauge(level: model.muted ? 0 : model.level, muted: model.muted)
                            .frame(width: 22, height: 22)
                    } else {
                        Button(action: sendTyped) {
                            Image(systemName: "arrow.up")
                                .font(.system(size: 13, weight: .bold))
                                .foregroundStyle(Color.black)
                                .frame(width: 24, height: 24)
                                .background(Circle().fill(ArbosTheme.text))
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.leading, 14)
                .padding(.trailing, 12)
                .frame(height: 43)
                .background(Capsule().fill(ArbosTheme.inputBg))
                .overlay(Capsule().strokeBorder(ArbosTheme.border, lineWidth: 1))
                CallDisc(symbol: model.muted ? "mic.slash" : "mic", filled: model.muted) {
                    model.muted.toggle()
                }
                .disabled(!model.phase.inCall)
                Button {
                    model.endCall()
                    dismiss()
                } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundStyle(Color.black)
                        .frame(width: 43, height: 43)
                        .background(Circle().fill(ArbosTheme.text))
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 36)
        }
        .padding(.bottom, 24)
    }

    /// Only when the orb alone cannot say it: nothing to call yet, or why
    /// the call ended. In a call the screen stays wordless.
    private var hint: String? {
        switch model.phase {
        case .idle: return "Tap to call"
        case .unconfigured(let why): return why
        case .failed(let reason): return reason
        case .connecting, .listening, .thinking, .speaking: return nil
        }
    }

    /// The identity to draw under the orb: the gateway's roster face when
    /// the call is scoped, else the chat's.
    private var onTheLine: ProjectIdentity {
        if let p = model.server.project {
            var face = chat.identity ?? ProjectIdentity(name: nil, icon: "", color: "").filled(key: "\(p.machine)/\(p.project)")
            face.name = p.name
            face.icon = p.icon
            return face
        }
        if let face = chat.identity { return face }
        var face = ProjectIdentity.defaults(key: settings.kernelTarget.stored, remote: true)
        face.name = projectName
        return face
    }

    private var projectName: String {
        chat.identity?.label ?? settings.kernelTarget.label.split(separator: "/").last.map(String.init) ?? "Arbos"
    }

    private var lastLine: TranscriptLine? {
        model.lines.last { $0.speaker != .system && !$0.text.isEmpty }
    }

    private func sendTyped() {
        guard !draft.trimmingCharacters(in: .whitespaces).isEmpty || !attachments.isEmpty else { return }
        model.sendTyped(draft, attachments: attachments)
        draft = ""
        attachments = []
    }

    /// The pickers behind the call's `+`.
    private var pickers: some View {
        Color.clear
            .frame(width: 0, height: 0)
            .photosPicker(isPresented: $showPhotos, selection: $photoItems, maxSelectionCount: 4, matching: .images)
            .onChange(of: photoItems) { _, items in
                guard !items.isEmpty else { return }
                Task {
                    for item in items {
                        if let file = await PendingAttachment.photo(item) { attachments.append(file) }
                    }
                    photoItems = []
                }
            }
            .fileImporter(isPresented: $showFiles, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
                guard case .success(let urls) = result else { return }
                for url in urls {
                    if let file = PendingAttachment.file(url) { attachments.append(file) }
                }
            }
    }

    /// Swipe down to pull the composer up; swipe up to put it away.
    private var pullGesture: some Gesture {
        DragGesture(minimumDistance: 24)
            .onEnded { value in
                guard abs(value.translation.width) < 80 else { return }
                if value.translation.height > 50 {
                    withAnimation(.spring(duration: 0.35)) { pulledDown = true }
                } else if value.translation.height < -50 {
                    typing = false
                    withAnimation(.spring(duration: 0.3)) { pulledDown = false }
                }
            }
    }

    /// The orb is the call's one control: tap to start when idle.
    private func tapOrb() {
        switch model.phase {
        case .unconfigured: showSettings = true
        case .idle, .failed: model.startCall()
        case .connecting, .listening, .thinking, .speaking: break
        }
    }

    #if DEBUG
    /// Launch arguments for review and scripted tests:
    /// `-previewChat 1` sends `-chatText` (or a default) to the chat;
    /// `-previewCall 1` starts a call at once (pair with `-injectWav`);
    /// `-pulledDown 1` opens with the composer row shown.
    private func previewIfAsked() async {
        let defaults = UserDefaults.standard
        DebugScreenshots.startIfAsked()
        if defaults.bool(forKey: "pulledDown") { pulledDown = true }
        if defaults.bool(forKey: "previewCall") {
            try? await Task.sleep(for: .milliseconds(300))
            model.startCall()
        }
        guard defaults.bool(forKey: "previewChat") else { return }
        try? await Task.sleep(for: .milliseconds(1500))
        chat.send(defaults.string(forKey: "chatText") ?? "What's left before I can merge?")
    }
    #endif
}

/// A 40 pt round control on the raised plate; `filled` inverts it.
struct CallDisc: View {
    let symbol: String
    var filled = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 18, weight: .regular))
                .foregroundStyle(filled ? Color.black : ArbosTheme.text)
                .frame(width: 40, height: 40)
                .background(Circle().fill(filled ? ArbosTheme.text : ArbosTheme.raised))
                .overlay(Circle().strokeBorder(filled ? Color.clear : ArbosTheme.border, lineWidth: 1))
        }
        .buttonStyle(.plain)
    }
}

/// The reference's small gauge inside the field: an arc that fills with
/// the microphone level, a slash when muted.
struct LevelGauge: View {
    let level: Float
    var muted = false

    var body: some View {
        ZStack {
            Circle()
                .trim(from: 0.12, to: 0.88)
                .stroke(ArbosTheme.border, style: StrokeStyle(lineWidth: 2, lineCap: .round))
                .rotationEffect(.degrees(90))
            Circle()
                .trim(from: 0.12, to: 0.12 + 0.76 * CGFloat(min(1, max(0, level))))
                .stroke(muted ? ArbosTheme.textDim : ArbosTheme.text, style: StrokeStyle(lineWidth: 2, lineCap: .round))
                .rotationEffect(.degrees(90))
                .animation(.easeOut(duration: 0.1), value: level)
            if muted {
                Image(systemName: "mic.slash")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(ArbosTheme.textDim)
            } else {
                Circle().fill(ArbosTheme.text).frame(width: 4, height: 4)
            }
        }
    }
}

/// The orb: one soft disc that moves with the live voice. White ink while
/// Jacob talks, the accent while Arbos speaks, a slow muted pulse while it
/// thinks, still and faint when idle. Its size follows the level (0.90 …
/// 1.0 of the frame, the reference's 189 pt at rest) with a soft halo;
/// no ring, no word — the colour and the motion are the state.
struct VoiceOrb: View {
    let level: Float
    let phase: CallViewModel.Phase
    @State private var shown: Float = 0

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 60)) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            let pulse = sin(t * 2 * .pi / 2.6) * 0.5 + 0.5
            let scale = discScale(pulse: pulse)
            ZStack {
                Circle()
                    .fill(ink.opacity(0.22))
                    .blur(radius: 26)
                    .scaleEffect(scale * 1.08)
                Circle()
                    .fill(
                        RadialGradient(
                            colors: [ink.opacity(0.98), ink.opacity(0.72)],
                            center: .init(x: 0.42, y: 0.34),
                            startRadius: 6,
                            endRadius: 110
                        )
                    )
                    .scaleEffect(scale)
            }
            .onChange(of: context.date) { _, _ in
                shown += (level - shown) * 0.5
            }
        }
        .animation(.easeInOut(duration: 0.35), value: phase)
    }

    private func discScale(pulse: Double) -> CGFloat {
        switch phase {
        case .listening, .speaking: return 0.90 + 0.10 * CGFloat(min(1, max(0, shown)))
        case .thinking, .connecting: return 0.90 + 0.04 * CGFloat(pulse)
        case .idle, .unconfigured, .failed: return 0.90
        }
    }

    private var ink: Color {
        switch phase {
        case .listening: return ArbosTheme.text
        case .speaking: return ArbosTheme.accent
        case .thinking, .connecting: return ArbosTheme.textMuted
        case .idle, .unconfigured: return ArbosTheme.textDim
        case .failed: return ArbosTheme.danger
        }
    }
}
