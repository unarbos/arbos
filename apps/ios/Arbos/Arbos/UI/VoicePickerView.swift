// The agent's call voice (§10.3): "Default (Marin)" first, then every voice
// from GET /voices, a checkmark on the chosen one and a small play button per
// row that plays the relay's sample. Saving is a tap: PATCH /projects/{id}
// {voice}, the row flips at once and the relay's answer is read back. Samples
// are fetched once and kept on disk (the relay marks them immutable) — the
// app never generates speech itself.

import AVFoundation
import SwiftUI

struct VoicePickerView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let project: Project
    @StateObject private var player = SamplePlayer()
    @State private var failed = false
    @State private var errorDetailsOpen = false

    private var current: Project { model.project(project.id) ?? project }
    private var catalog: VoiceCatalog? { model.voiceCatalog }
    private var defaultVoice: Voice? { catalog.flatMap { c in c.voices.first { $0.id == c.defaultId } } }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    if let catalog {
                        voiceRow(id: nil, name: "Default (\(defaultVoice?.name ?? catalog.defaultId.capitalized))",
                                 // The named voice's row carries its description; say what Default means instead of repeating it.
                                 description: "What the relay uses when nothing is chosen", sample: defaultVoice?.sampleURL, selected: current.voice == nil)
                        ForEach(catalog.voices) { v in
                            voiceRow(id: v.id, name: v.name, description: v.description, sample: v.sampleURL, selected: current.voice == v.id)
                        }
                    } else if failed {
                        // The sheet's note (the shared element) where the list would be.
                        ErrorNote(id: "voices-error",
                                  error: UserError.of("load the voices", .offline(deviceOffline: false), "GET /voices failed"),
                                  detailsOpen: $errorDetailsOpen)
                            .listRowBackground(Color.clear)
                            .listRowInsets(EdgeInsets())
                    } else {
                        HStack(spacing: 10) {
                            ProgressView()
                            Text("Loading voices…").foregroundStyle(theme.muted)
                        }
                        .listRowBackground(theme.card)
                    }
                } footer: {
                    Text("How \(current.name) sounds on calls. Tap ▶ to hear a sample; tap a row to choose it.")
                        .font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                }
            }
            .scrollContentBackground(.hidden)
            .background(theme.bg)
            .navigationTitle("Voice")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } }
            }
            .task {
                await model.loadVoices()
                failed = model.voiceCatalog == nil
            }
            .onDisappear { player.stop() }
        }
        .presentationDetents([.large])
    }

    private func voiceRow(id: String?, name: String, description: String?, sample: URL?, selected: Bool) -> some View {
        // The Default row shares the default voice's sample, so rows are keyed, not URLs.
        let key = id ?? "default"
        return HStack(spacing: 12) {
            Button {
                guard let sample else { return }
                player.toggle(sample, key: key)
            } label: {
                ZStack {
                    Circle().fill(theme.isDark ? Color(hex: "#2C2C2C")! : Color(hex: "#F1F1F3")!)
                    if player.isLoading(key) {
                        ProgressView().controlSize(.small)
                    } else {
                        Image(systemName: player.isPlaying(key) ? "stop.fill" : "play.fill")
                            .font(theme.font(12, weight: .semibold))
                            .foregroundStyle(theme.text)
                            .offset(x: player.isPlaying(key) ? 0 : 1)
                    }
                }
                .frame(width: 32, height: 32)
                .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .disabled(sample == nil)
            .opacity(sample == nil ? 0.4 : 1)
            .accessibilityLabel(player.isPlaying(key) ? "Stop sample" : "Play \(name) sample")

            Button {
                model.setVoice(project.id, id)
            } label: {
                HStack(spacing: 10) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(name).font(theme.font(Theme.body, weight: selected ? .semibold : .regular)).foregroundStyle(theme.text)
                        if let description, !description.isEmpty {
                            Text(description).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(2)
                        }
                    }
                    Spacer()
                    if selected {
                        Image(systemName: "checkmark").font(.system(size: 15, weight: .semibold)).foregroundStyle(theme.text)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("\(name)\(selected ? ", selected" : "")")
            .accessibilityAddTraits(selected ? [.isSelected] : [])
        }
        .padding(.vertical, 2)
        .listRowBackground(theme.card)
    }
}

/// Plays one sample at a time from a disk cache (`Caches/voice-samples/<sha>.mp3`).
@MainActor
final class SamplePlayer: NSObject, ObservableObject, AVAudioPlayerDelegate {
    @Published private(set) var playingKey: String?
    @Published private(set) var loadingKey: String?
    private var player: AVAudioPlayer?
    private var fetch: Task<Void, Never>?

    func isPlaying(_ key: String) -> Bool { playingKey == key }
    func isLoading(_ key: String) -> Bool { loadingKey == key }

    func toggle(_ url: URL, key: String) {
        if playingKey == key || loadingKey == key { stop(); return }
        stop()
        loadingKey = key
        fetch = Task { [weak self] in
            guard let self else { return }
            do {
                let file = try await SamplePlayer.cached(url)
                guard !Task.isCancelled, loadingKey == key else { return }
                try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .default, options: [])
                try? AVAudioSession.sharedInstance().setActive(true)
                let p = try AVAudioPlayer(contentsOf: file)
                p.delegate = self
                p.prepareToPlay()
                player = p
                loadingKey = nil
                playingKey = key
                p.play()
            } catch {
                loadingKey = nil
            }
        }
    }

    func stop() {
        fetch?.cancel()
        fetch = nil
        player?.stop()
        player = nil
        playingKey = nil
        loadingKey = nil
    }

    nonisolated func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        Task { @MainActor in
            self.playingKey = nil
            self.player = nil
        }
    }

    /// Download once; the relay serves samples with immutable cache headers, so a
    /// file on disk is good for ever. Falls back to URLCache for the HTTP layer too.
    private static func cached(_ url: URL) async throws -> URL {
        let dir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("voice-samples", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let name = url.absoluteString.data(using: .utf8).map { d -> String in
            var h: UInt64 = 0xcbf29ce484222325
            for b in d { h ^= UInt64(b); h = h &* 0x100000001b3 }
            return String(h, radix: 16)
        } ?? UUID().uuidString
        let file = dir.appendingPathComponent(name + "." + (url.pathExtension.isEmpty ? "mp3" : url.pathExtension))
        if FileManager.default.fileExists(atPath: file.path) { return file }
        var req = URLRequest(url: url)
        req.cachePolicy = .returnCacheDataElseLoad
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard (resp as? HTTPURLResponse).map({ (200..<300).contains($0.statusCode) }) ?? true, !data.isEmpty else {
            throw URLError(.badServerResponse)
        }
        try data.write(to: file, options: .atomic)
        return file
    }
}
