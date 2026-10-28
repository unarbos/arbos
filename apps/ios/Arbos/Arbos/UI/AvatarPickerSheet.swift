// The avatar picker sheet (D92 amendment, Jacob 2026-10-23): Grok Bot's avatar picker for an
// agent's picture — a tab pill Bot | Generate | Upload with Reset at the right, a body per tab,
// and a footer Cancel | Set avatar. Nothing changes until "Set avatar".
//
// Bot: the sixteen D92 (v2) leaves as their 4 × 4 sheet, every leaf neutral and the selected one
// in its hue (pastel disc, darker leaf); the ten hues in two rows of five. The per-id
// automatic default stays: Reset goes back to it. Generate: a short prompt → the relay paints one
// picture (`POST /projects/{id}/avatar/generate`) → previewed here; Set keeps it. Upload: Photos
// (images only), uploaded as the picture master on Set — `PictureMaster`: a 1024 px square,
// PNG for a flat mark, JPEG q92 for a photo (Jacob, 2026-10-26: a 4000 px mark came out
// jagged through the old 512 px JPEG). Groups and servers open the same sheet with the same
// three tabs (their own `/avatar/generate` route and blob slot).
//
// Grok's sizes from the 2x reference (612 × 794 px → 306 × 397 pt), inside the shipped
// bottom-sheet frame: a 44 pt bar with a hairline, the Create sheet's 33 / 40 / 56 leaf grid,
// 23 pt swatches on a 40 pt pitch, two equal footer pills 8 pt apart (44 pt tall here, the
// touch minimum — the same deviation as the Create sheet).
//
// The Create sheets (`CreateEntityView`, Jacob 2026-10-25) draw the **same Generate and Upload
// bodies** through `AvatarSourceBody` and `AvatarSourceState` — `Upload | Generate` only, no
// Bot tab, no Reset — and generate into my own slot (`POST /me/avatar/generate`) until the
// entity exists.
//
// My own profile picture (Jacob, 2026-10-25) opens this sheet too — `AvatarTarget.me`:
// Generate | Upload with Reset (people are D79 coloured initials, so no Bot tab); Generate goes
// through the same `/me/avatar/generate`, Set sends `PUT /me/avatar {image_key}` or the JPEG,
// Reset is `DELETE /me/avatar`.

import ArbosFFI
import PhotosUI
import SwiftUI
import UIKit

/// Whose picture the sheet edits.
enum AvatarTarget: Equatable {
    case agent(Id)
    case group(String)
    case server(String)
    /// My own profile picture (`/me/avatar…`; Jacob, 2026-10-25).
    case me
}

/// Which body is up.
enum AvatarPickerTab: String, CaseIterable, Identifiable {
    var id: String { rawValue }
    case bot = "Bot"
    case generate = "Generate"
    case upload = "Upload"
}

/// Grok's dark sheet palette, shared by the picker sheet and the Create sheets.
enum AvatarSheetPalette {
    static let sheetBg = Color(red: 0x1F / 255, green: 0x1F / 255, blue: 0x1F / 255)
    static let line = Color(red: 0x35 / 255, green: 0x35 / 255, blue: 0x35 / 255)
    static let ring = Color(red: 0x50 / 255, green: 0x50 / 255, blue: 0x50 / 255)
    static let placeholder = Color(red: 0x93 / 255, green: 0x93 / 255, blue: 0x93 / 255)
    static let labelMuted = Color(red: 0x9A / 255, green: 0x9A / 255, blue: 0x9A / 255)
    static let pillBg = Color(red: 0x27 / 255, green: 0x27 / 255, blue: 0x27 / 255)
    static let mutedDisc = Color(red: 0x3A / 255, green: 0x3A / 255, blue: 0x3A / 255)
    static let mutedInk = Color(red: 0x8A / 255, green: 0x8A / 255, blue: 0x8A / 255)
    static let disabledBg = Color(red: 0x3B / 255, green: 0x3B / 255, blue: 0x3B / 255)
    static let disabledText = Color(red: 0x73 / 255, green: 0x73 / 255, blue: 0x73 / 255)
    static let setBg = Color(white: 0xE5 / 255)
    static let title = Color(white: 0xF5 / 255)
}

/// The pending Upload / Generate choice — one object for the picker sheet and the Create
/// sheets, so the two draw and behave the same.
@MainActor
final class AvatarSourceState: ObservableObject {
    @Published var prompt = ""
    @Published var generating = false
    @Published var generatedKey: String?
    /// The last failure, drawn by the shared `ErrorNote` at the top of the sheet's body (Jacob,
    /// 2026-10-26, [feedback error-messages-ios-2026-10-26]): one sentence, Try again, Details.
    @Published var error: UserError?
    @Published var detailsOpen = false
    /// The sentence alone (the older name, read-only).
    var errorText: String? { error?.sentence }
    /// When the running generation started (the status line counts the seconds).
    @Published var startedAt: Date?
    @Published var photoItem: PhotosPickerItem?

    /// One generation: progress on, then the key or the (sentence, detail) of the failure.
    /// Both sheets call this so the wording is the same everywhere.
    func run(_ work: @escaping () async throws -> String) {
        guard !generating else { return }
        generating = true
        startedAt = Date()
        error = nil
        detailsOpen = false
        Task { @MainActor in
            do {
                generatedKey = try await work()
            } catch {
                self.error = Self.failure(error)
            }
            generating = false
            startedAt = nil
        }
    }

    /// What the sheet says when a generation failed — the shared classifier under the verb
    /// "generate the image": a relay timeout / refusal keeps its class, a client-side timeout
    /// reads "The server took too long.", no network "You're offline."; the raw words ride in
    /// `raw` for Details / Copy, never in the sentence.
    static func failure(_ error: Error) -> UserError {
        UserError.classify("generate the image", error)
    }

    /// The older pair, for callers that still read it: (sentence, raw words).
    static func failureWords(_ error: Error) -> (String, String?) {
        let e = failure(error)
        return (e.sentence, e.raw.isEmpty ? nil : e.raw)
    }

    /// Seconds since Generate was pressed, while it runs.
    func generatingFor(_ now: Date) -> Int? {
        guard generating, let startedAt else { return nil }
        return max(0, Int(now.timeIntervalSince(startedAt)))
    }
    @Published var uploadPreview: UIImage?
    /// The encoded master of the chosen photo (PNG or JPEG — `PictureMaster.mime` tells which;
    /// the name predates the PNG case).
    @Published var uploadJpeg: Data?

    /// The entity about to be created gets this (Jacob, 2026-10-25): the chosen JPEG, the
    /// generated key in my slot, or nothing (→ the neutral initial).
    func draftPicture(for tab: AvatarPickerTab) -> DraftPicture? {
        switch tab {
        case .upload: return uploadJpeg.map(DraftPicture.jpeg)
        case .generate: return generatedKey.map { DraftPicture.generated(key: $0) }
        case .bot: return nil
        }
    }

    /// A Photos pick landed: square it at the master size and keep the encoded master for the
    /// create / Set (`PictureMaster`).
    func tookPhoto(_ item: PhotosPickerItem?) async {
        guard let item,
              let data = try? await item.loadTransferable(type: Data.self),
              let img = UIImage(data: data) else { return }
        let square = AvatarPickerSheet.square(img, side: PictureMaster.side)
        let encoded = PictureMaster.encode(square)
        // The 114 pt preview draws a 384 px ImageIO thumbnail of the master (≈ 1 : 1 at 3 ×), not
        // the 1024 px square through the layer's linear filter.
        uploadPreview = encoded.flatMap(AvatarImageCache.decode) ?? square
        uploadJpeg = encoded
        error = nil
    }
}

/// The two option capsules (`Upload | Generate` on a Create sheet; the picker sheet adds Bot).
struct AvatarTabPill: View {
    let tabs: [AvatarPickerTab]
    @Binding var tab: AvatarPickerTab
    var onChange: (() -> Void)? = nil

    var body: some View {
        HStack(spacing: 2) {
            ForEach(tabs, id: \.self) { t in
                Button {
                    tab = t
                    onChange?()
                } label: {
                    Text(t.rawValue)
                        .font(.system(size: 13, weight: .medium))
                        .foregroundStyle(tab == t ? AvatarSheetPalette.title : AvatarSheetPalette.labelMuted)
                        .padding(.horizontal, 11)
                        .frame(height: 28)
                        .background(Capsule().fill(tab == t ? AvatarSheetPalette.pillBg : .clear))
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(t.rawValue + (tab == t ? ", selected" : ""))
            }
        }
    }
}

/// The Generate / Upload tab bodies, shared by the picker sheet and the Create sheets. `preview`
/// (the 114 pt disc) is drawn inside the body when given; a Create sheet shows its own hero
/// preview above the name field and passes none. `keepsLine` names the action that keeps a
/// generated picture ("Set avatar" / "Create").
struct AvatarSourceBody<Preview: View>: View {
    @Environment(\.theme) private var theme
    @ObservedObject var state: AvatarSourceState
    let tab: AvatarPickerTab
    var keepsLine = "Set avatar keeps this one."
    var uploadsLine = "Set avatar uploads it."
    var generate: () -> Void
    @ViewBuilder var preview: () -> Preview
    @FocusState private var promptFocused: Bool

    private var canGenerate: Bool { !state.prompt.trimmingCharacters(in: .whitespaces).isEmpty && !state.generating }
    /// The status line never carries a failure (that is the note at the top of the body):
    /// progress, the keep hint, or the prompt hint.
    private func statusLine(_ now: Date) -> String {
        if let s = state.generatingFor(now) { return "Painting a picture — \(s) s" }
        if state.generatedKey != nil { return keepsLine }
        return "A few words about the picture you want."
    }

    var body: some View {
        switch tab {
        case .generate: generateBody
        case .upload: uploadBody
        case .bot: EmptyView()
        }
    }

    private var generateBody: some View {
        VStack(spacing: 0) {
            TextField("", text: $state.prompt, prompt: Text("A fox reading by a lamp").foregroundStyle(AvatarSheetPalette.placeholder))
                .font(.system(size: 14))
                .foregroundStyle(AvatarSheetPalette.title)
                .autocorrectionDisabled()
                .focused($promptFocused)
                .submitLabel(.go)
                .onSubmit(go)
                .padding(.horizontal, 12)
                .frame(height: 35)
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(AvatarSheetPalette.line, lineWidth: 1))
                .padding(.horizontal, 16.5)
                .accessibilityIdentifier("avatar-generate-prompt")
            Button(action: go) {
                Text(pillLabel)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(canGenerate ? AvatarSheetPalette.title : AvatarSheetPalette.disabledText)
                    .padding(.horizontal, 18)
                    .frame(height: 28)
                    .background(Capsule().fill(canGenerate ? AvatarSheetPalette.pillBg : AvatarSheetPalette.disabledBg))
            }
            .buttonStyle(.plain)
            .disabled(!canGenerate)
            .padding(.top, 12)
            .accessibilityIdentifier("avatar-generate-go")
            // The preview slot: the picture, dimmed under a spinner while painting. A failure is
            // the shared note at the top of the body (`errorNote`), never anything near the footer.
            preview()
                .opacity(state.generating ? 0.4 : 1)
                .overlay {
                    if state.generating {
                        ProgressView().controlSize(.large).tint(theme.accent)
                            .accessibilityIdentifier("avatar-generate-spinner")
                    }
                }
            // One line, never more.
            TimelineView(.periodic(from: .now, by: 1)) { ctx in
                Text(statusLine(ctx.date))
                    .font(.system(size: 12))
                    .foregroundStyle(AvatarSheetPalette.placeholder)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .accessibilityIdentifier("avatar-generate-status")
                    .padding(.horizontal, 16.5)
                    .padding(.top, 12)
            }
        }
        .frame(maxWidth: .infinity)
    }

    /// The note for the top of a sheet's body (the picker's and the Create sheets'): inset like
    /// the field, `ErrorNote.gap` under it. Try again re-sends the prompt.
    @ViewBuilder
    static func errorNote(_ state: AvatarSourceState, generate: @escaping () -> Void) -> some View {
        if let error = state.error {
            ErrorNote(id: "avatar-generate-error", error: error,
                      detailsOpen: Binding(get: { state.detailsOpen }, set: { state.detailsOpen = $0 }),
                      onTryAgain: generate)
                .padding(.horizontal, 16.5)
                .padding(.bottom, ErrorNote.gap)
        }
    }

    private var pillLabel: String {
        if state.generating { return "Generating…" }
        return state.generatedKey == nil ? "Generate" : "Generate again"
    }

    private func go() {
        guard canGenerate else { return }
        promptFocused = false
        generate()
    }

    private var uploadBody: some View {
        VStack(spacing: 0) {
            preview()
            PhotosPicker(selection: $state.photoItem, matching: .images) {
                Text(state.uploadJpeg == nil ? "Choose a photo…" : "Choose another…")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(AvatarSheetPalette.title)
                    .padding(.horizontal, 18)
                    .frame(height: 28)
                    .background(Capsule().fill(AvatarSheetPalette.pillBg))
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("avatar-upload-choose")
            Text(state.uploadJpeg == nil ? "A picture from Photos; kept at up to 1024 px." : uploadsLine)
                .font(.system(size: 12))
                .foregroundStyle(AvatarSheetPalette.placeholder)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 16.5)
                .padding(.top, 12)
        }
        .frame(maxWidth: .infinity)
        .onChange(of: state.photoItem) { _, item in
            Task { await state.tookPhoto(item) }
        }
    }
}

struct AvatarPickerSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let target: AvatarTarget
    var initialTab: AvatarPickerTab = .bot

    @State private var tab: AvatarPickerTab = .bot
    @State private var pickedLeaf: Int?
    @State private var pickedColor: Int?
    @State private var reset = false
    @StateObject private var source = AvatarSourceState()
    @State private var saving = false
    /// View (Jacob, 2026-10-25): the picture as it is now, full-size in the media viewer.
    @State private var viewer: MediaViewerState?
    private var generatedKey: String? { source.generatedKey }
    private var uploadJpeg: Data? { source.uploadJpeg }
    private var uploadPreview: UIImage? { source.uploadPreview }
    private var generating: Bool { source.generating }
    private var errorText: String? {
        get { source.errorText }
        nonmutating set { if newValue == nil { source.error = nil } }
    }

    // Grok's dark sheet palette, as the Create sheet has it.
    private let sheetBg = Color(red: 0x1F / 255, green: 0x1F / 255, blue: 0x1F / 255)
    private let line = Color(red: 0x35 / 255, green: 0x35 / 255, blue: 0x35 / 255)
    private let ring = Color(red: 0x50 / 255, green: 0x50 / 255, blue: 0x50 / 255)
    private let placeholder = Color(red: 0x93 / 255, green: 0x93 / 255, blue: 0x93 / 255)
    private let labelMuted = Color(red: 0x9A / 255, green: 0x9A / 255, blue: 0x9A / 255)
    private let pillBg = Color(red: 0x27 / 255, green: 0x27 / 255, blue: 0x27 / 255)
    private let mutedDisc = Color(red: 0x3A / 255, green: 0x3A / 255, blue: 0x3A / 255)
    private let mutedInk = Color(red: 0x8A / 255, green: 0x8A / 255, blue: 0x8A / 255)
    private let disabledBg = Color(red: 0x3B / 255, green: 0x3B / 255, blue: 0x3B / 255)
    private let disabledText = Color(red: 0x73 / 255, green: 0x73 / 255, blue: 0x73 / 255)
    private let setBg = Color(white: 0xE5 / 255)
    private let title = Color(white: 0xF5 / 255)

    /// The tabs per target: agents, groups and servers get all three (Jacob, 2026-10-24: the
    /// group / server picker had lost Generate); my own picture has no leaf — people are D79
    /// coloured initials — so it gets Generate | Upload (Jacob, 2026-10-25). Reset sits beside
    /// them in the bar for every target.
    static func tabs(for target: AvatarTarget) -> [AvatarPickerTab] {
        switch target {
        case .agent, .group, .server: return [.bot, .generate, .upload]
        case .me: return [.generate, .upload]
        }
    }
    private var tabs: [AvatarPickerTab] { Self.tabs(for: target) }
    private var project: Project? { if case .agent(let id) = target { return model.project(id) } else { return nil } }
    private var people: PeopleChat? { if case .group(let id) = target { return model.peopleChats.first { $0.id == id } } else { return nil } }
    private var server: Server? { if case .server(let id) = target { return model.servers.first { $0.id == id } } else { return nil } }
    /// The blob slot a generated picture lands in (where the preview fetches it from).
    private var slot: String {
        switch target {
        case .agent(let id): return id
        case .group(let id): return "\(peopleSlot):\(id)"
        case .server(let id): return "servers:\(id)"
        case .me: return "users:\(model.user?.id ?? "")"
        }
    }

    /// The entity's leaf + colour as it is now; the automatic pair under Reset.
    private var currentPick: LeafPick {
        switch target {
        case .agent(let id):
            if reset || project == nil { return LeafArt.pick(seed: id) }
            return LeafArt.pick(project!)
        case .group(let id):
            if reset { return LeafArt.pick(seed: id) }
            return people?.leafPick ?? LeafArt.pick(seed: id)
        case .server(let id):
            if reset { return LeafArt.pick(seed: id) }
            return server?.leafPick ?? LeafArt.pick(seed: id)
        case .me:
            // People have no leaf; the Bot tab never shows for me.
            return LeafArt.pick(seed: model.user?.id ?? "me")
        }
    }
    private var pick: LeafPick {
        LeafPick(leaf: UInt8(pickedLeaf ?? Int(currentPick.leaf)), color: UInt8(pickedColor ?? Int(currentPick.color)))
    }
    private var canReset: Bool {
        switch target {
        case .agent: return project.map { $0.avatarLeaf != nil || $0.avatarLeafColor != nil || $0.imageKey != nil } ?? false
        case .group: return people.map { $0.leafPick != nil || $0.imageKey != nil } ?? false
        case .server: return server.map { $0.leafPick != nil || $0.imageKey != nil } ?? false
        // Only an Arbos-uploaded picture resets (the GitHub one is already the default).
        case .me: return model.hasCustomAvatar
        }
    }
    private var dirty: Bool {
        if reset { return true }
        switch tab {
        case .bot: return pickedLeaf != nil || pickedColor != nil
        case .generate: return generatedKey != nil
        case .upload: return uploadJpeg != nil
        }
    }
    private var canSet: Bool { dirty && !generating && !saving }

    var body: some View {
        VStack(spacing: 0) {
            bar
            ScrollView(showsIndicators: false) {
                // The note first (Jacob 2026-10-26): under the bar, before the field; the body
                // below moves down, the footer never moves.
                AvatarSourceBody<EmptyView>.errorNote(source, generate: generate)
                    .padding(.top, tab == .generate ? 12 : 0)
                switch tab {
                case .bot: botBody
                case .generate, .upload:
                    AvatarSourceBody(state: source, tab: tab, generate: generate) {
                        preview.padding(.top, tab == .upload ? 24 : 16).padding(.bottom, tab == .upload ? 16 : 0)
                    }
                    .padding(.top, tab == .generate ? 16 : 0)
                    .padding(.bottom, 24)
                }
            }
            .scrollDismissesKeyboard(.interactively)
            footer
        }
        .background(sheetBg.ignoresSafeArea())
        .presentationDetents([.height(397 + 44)])
        .presentationDragIndicator(.visible)
        .onAppear {
            tab = tabs.contains(initialTab) ? initialTab : (tabs.first ?? .bot)
            // Rig: a prefilled prompt runs Generate at once; `ARBOS_VIEW_PICTURE=1` presses View.
            if tab == .generate, let text = DebugHooks.avatarPrompt, !text.isEmpty {
                source.prompt = text
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { generate() }
            }
            if DebugHooks.viewPicture, let picture = Self.viewablePicture(target: target, model: model) {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
                    viewer = MediaViewerState(items: [MediaAlbum.picture(name: picture.name, url: picture.url)], index: 0)
                }
            }
        }
        .onChange(of: source.uploadJpeg) { _, jpeg in
            if jpeg != nil { reset = false }
        }
        .fullScreenCover(item: $viewer) { v in
            MediaViewer(album: v.items, index: v.index, onClose: { viewer = nil })
                .environmentObject(model)
                .environment(\.theme, theme)
        }
    }

    /// The target's current picture, when it has one — what View opens. Leaf / letter avatars
    /// have nothing to view, so the bar shows no View.
    static func viewablePicture(target: AvatarTarget, model: AppModel) -> (name: String, url: String)? {
        switch target {
        case .agent(let id):
            guard let p = model.project(id), let url = model.projectImageURL(p)?.absoluteString, !url.isEmpty else { return nil }
            return (p.name, url)
        case .group(let id):
            guard let c = model.peopleChats.first(where: { $0.id == id }), let key = c.imageKey,
                  let url = model.blobURL(projectId: "\(peopleSlot):\(id)", key: key) else { return nil }
            return (c.title.isEmpty ? "Group" : c.title, url.absoluteString)
        case .server(let id):
            guard let s = model.servers.first(where: { $0.id == id }), let key = s.imageKey,
                  let url = model.blobURL(projectId: "servers:\(id)", key: key) else { return nil }
            return (s.name, url.absoluteString)
        case .me:
            guard let u = model.user, let url = u.avatarUrl, !url.isEmpty else { return nil }
            return (u.name ?? u.login, url)
        }
    }

    // MARK: bar — Bot | Generate | Upload …… Reset

    private var bar: some View {
        HStack(spacing: 2) {
            AvatarTabPill(tabs: tabs, tab: $tab) { errorText = nil }
            Spacer()
            if let picture = Self.viewablePicture(target: target, model: model) {
                Button {
                    viewer = MediaViewerState(items: [MediaAlbum.picture(name: picture.name, url: picture.url)], index: 0)
                } label: {
                    Text("View")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundStyle(labelMuted)
                        .padding(.horizontal, 6)
                        .frame(height: 28)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("avatar-view")
            }
            Button {
                reset = true
                pickedLeaf = nil
                pickedColor = nil
                source.uploadJpeg = nil
                source.uploadPreview = nil
                source.generatedKey = nil
                errorText = nil
            } label: {
                Text("Reset")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(reset ? title : (canReset ? labelMuted : disabledText))
                    .padding(.horizontal, 6)
                    .frame(height: 28)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(!canReset)
        }
        .padding(.horizontal, 12)
        .frame(height: 44)
        .overlay(alignment: .bottom) { Rectangle().fill(line).frame(height: 1) }
    }

    // MARK: Bot

    /// Nine leaves, 5 / 4, on a 56 pt pitch: 33 pt discs, only the selected one tinted; then the
    /// ten colours in two rows of five on a 40 pt pitch (23 pt discs, a 31 pt ring on the pick).
    /// The 4 × 4 leaf sheet (v2) on a 48 pt pitch — 30 pt discs, every leaf neutral and the
    /// selected one in its hue — then the ten hues on a 36 pt pitch (21 pt discs, 29 pt ring).
    private var botBody: some View {
        VStack(spacing: 0) {
            VStack(spacing: 48 - 36) {
                ForEach(0..<4, id: \.self) { r in
                    HStack(spacing: 48 - 36) {
                        ForEach(r * 4..<r * 4 + 4, id: \.self) { i in
                            let selected = Int(pick.leaf) == i
                            Button {
                                pickedLeaf = i
                                reset = false
                            } label: {
                                ZStack {
                                    Circle().fill(selected ? LeafArt.colors[Int(pick.color)] : mutedDisc)
                                    LeafShape(index: i)
                                        .fill(selected ? LeafArt.ink(pick.color) : mutedInk, style: FillStyle(eoFill: true))
                                        .frame(width: 30 * LeafArt.scale, height: 30 * LeafArt.scale)
                                }
                                .frame(width: 30, height: 30)
                                .frame(width: 36, height: 36)
                                .overlay(Circle().strokeBorder(selected ? ring : .clear, lineWidth: 1))
                                .contentShape(Circle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel(LeafArt.names[i] + (selected ? ", selected" : ""))
                        }
                    }
                }
            }
            .padding(.top, 84 - 44 - 18)
            VStack(spacing: 36 - 29) {
                ForEach([Array(0..<5), Array(5..<10)], id: \.self) { row in
                    HStack(spacing: 36 - 29) {
                        ForEach(row, id: \.self) { i in
                            let selected = Int(pick.color) == i
                            Button {
                                pickedColor = i
                                reset = false
                            } label: {
                                Circle().fill(LeafArt.colors[i]).frame(width: 21, height: 21)
                                    .frame(width: 29, height: 29)
                                    .overlay(Circle().strokeBorder(selected ? ring : .clear, lineWidth: 1))
                                    .contentShape(Circle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel(LeafArt.colorNames[i] + (selected ? ", selected" : ""))
                        }
                    }
                }
            }
            .padding(.top, 282 - 228 - 18 - 14.5)
        }
        .frame(maxWidth: .infinity)
        .padding(.bottom, 24)
    }

    // MARK: Generate

    private func generate() {
        let text = source.prompt.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty, !source.generating else { return }
        reset = false
        let target = target
        let model = model
        source.run {
            switch target {
            case .agent(let id): return try await model.generateProjectAvatar(projectId: id, prompt: text)
            case .group(let id): return try await model.generateGroupAvatar(chatId: id, prompt: text)
            case .server(let id): return try await model.generateServerAvatar(id, prompt: text)
            case .me: return try await model.generateMyAvatar(prompt: text)
            }
        }
    }

    /// The round 114 pt preview: the pending choice, else the picture as it is now.
    @ViewBuilder
    private var preview: some View {
        if let uploadPreview, tab == .upload {
            Image(uiImage: uploadPreview).resizable().scaledToFill()
                .frame(width: 114, height: 114).clipShape(Circle())
        } else if tab == .generate, let generatedKey,
                  let url = model.blobURL(projectId: slot, key: generatedKey) {
            AsyncImage(url: url) { phase in
                if let image = phase.image {
                    image.resizable().scaledToFill()
                } else {
                    ZStack {
                        Circle().fill(mutedDisc)
                        if phase.error != nil {
                            // The fixture's "generated" key has no blob behind it: the picture as it is.
                            currentPicture
                        } else {
                            ProgressView().tint(placeholder)
                        }
                    }
                }
            }
            .frame(width: 114, height: 114)
            .clipShape(Circle())
        } else if reset, case .me = target {
            // Back to the GitHub picture (unknown here) or the coloured initials.
            PersonAvatar(url: nil, handle: model.user?.login ?? "?", size: 114,
                         name: model.user?.name, id: model.user?.id,
                         colorIndex: model.user?.avatarColorIndexEffective)
        } else if reset {
            LeafAvatar(pick: currentPick, size: 114)
        } else {
            currentPicture
        }
    }

    /// The entity's picture as it is now (no pending choice).
    @ViewBuilder
    private var currentPicture: some View {
        switch target {
        case .agent:
            if let project {
                AgentAvatar(project: project, theme: theme, size: 114, still: true)
            } else {
                LeafAvatar(pick: currentPick, size: 114)
            }
        case .group(let id):
            if let key = people?.imageKey, let url = model.blobURL(projectId: "\(peopleSlot):\(id)", key: key) {
                AsyncImage(url: url) { phase in
                    if let image = phase.image { image.resizable().scaledToFill() } else { Circle().fill(mutedDisc) }
                }
                .frame(width: 114, height: 114).clipShape(Circle())
            } else if let leaf = people?.leafPick {
                LeafAvatar(pick: leaf, size: 114)
            } else {
                LeafAvatar(pick: currentPick, size: 114).opacity(0.55)
            }
        case .server:
            if let server {
                ServerAvatar(server: server, size: 114)
            } else {
                LeafAvatar(pick: currentPick, size: 114).opacity(0.55)
            }
        case .me:
            PersonAvatar(url: model.user?.avatarUrl, handle: model.user?.login ?? "?", size: 114,
                         name: model.user?.name, id: model.user?.id,
                         colorIndex: model.user?.avatarColorIndexEffective)
        }
    }

    // MARK: footer — Cancel | Set avatar

    private var footer: some View {
        HStack(spacing: 8) {
            Button { dismiss() } label: {
                Text("Cancel").font(.system(size: 15, weight: .semibold)).foregroundStyle(title)
                    .frame(maxWidth: .infinity).frame(height: 44).background(Capsule().fill(pillBg))
            }
            .buttonStyle(.plain)
            Button(action: setAvatar) {
                Text(saving ? "Saving…" : "Set avatar").font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(canSet ? Color(red: 0x1F / 255, green: 0x1F / 255, blue: 0x1F / 255) : disabledText)
                    .frame(maxWidth: .infinity).frame(height: 44)
                    .background(Capsule().fill(canSet ? setBg : disabledBg))
            }
            .buttonStyle(.plain)
            .disabled(!canSet)
        }
        .padding(.horizontal, 12)
        .padding(.top, 6)
        .padding(.bottom, 10)
        .overlay(alignment: .top) { Rectangle().fill(line).frame(height: 1) }
    }

    private func setAvatar() {
        guard canSet else { return }
        saving = true
        Task {
            var ok = true
            switch target {
            case .agent(let id):
                if reset {
                    model.pickAvatarLeaf(id, leaf: nil, color: nil)
                } else {
                    switch tab {
                    case .bot: model.pickAvatarLeaf(id, leaf: Int(pick.leaf), color: Int(pick.color))
                    case .generate: if let generatedKey { ok = await model.setProjectImageKey(projectId: id, key: generatedKey) }
                    case .upload: if let uploadJpeg { ok = await model.setProjectImage(projectId: id, jpeg: uploadJpeg) }
                    }
                }
            case .group(let id):
                if reset {
                    ok = await model.setGroupLeaf(chatId: id, pick: nil)
                } else {
                    switch tab {
                    case .bot: ok = await model.setGroupLeaf(chatId: id, pick: pick)
                    case .upload: if let uploadJpeg { ok = await model.setGroupImage(chatId: id, jpeg: uploadJpeg) }
                    case .generate: if let generatedKey { ok = await model.setGroupImageKey(chatId: id, key: generatedKey) }
                    }
                }
            case .server(let id):
                if reset {
                    ok = await model.setServerLeaf(id, pick: nil)
                } else {
                    switch tab {
                    case .bot: ok = await model.setServerLeaf(id, pick: pick)
                    case .upload: if let uploadJpeg { ok = await model.setServerImage(id, jpeg: uploadJpeg) }
                    case .generate: if let generatedKey { ok = await model.setServerImageKey(id, key: generatedKey) }
                    }
                }
            case .me:
                // Reset = the GitHub picture again (`DELETE /me/avatar`); a photo goes up as
                // `PUT /me/avatar`; a generated preview is set by its key.
                if reset {
                    ok = await model.removeUserAvatar()
                } else {
                    switch tab {
                    case .bot: break
                    case .upload: if let uploadJpeg { ok = await model.setUserAvatar(jpeg: uploadJpeg) }
                    case .generate: if let generatedKey { ok = await model.setUserAvatarKey(generatedKey) }
                    }
                }
            }
            saving = false
            if ok { dismiss() }
        }
    }

    /// The centre square of `image`, `side` **pixels** on a side (scale 1 — not the screen's 3 ×,
    /// which made a "512" picture 1536 px), drawn with high-quality interpolation so a 4000 px
    /// export shrinks through an area filter, never point-sampled. A smaller picture is not
    /// enlarged.
    static func square(_ image: UIImage, side: CGFloat) -> UIImage {
        let s = min(image.size.width, image.size.height)
        let rect = CGRect(x: (image.size.width - s) / 2, y: (image.size.height - s) / 2, width: s, height: s)
        let sourcePx = s * image.scale
        let out = min(side, max(1, sourcePx.rounded(.down)))
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = false
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: out, height: out), format: format)
        return renderer.image { ctx in
            ctx.cgContext.interpolationQuality = .high
            if let cg = image.cgImage?.cropping(to: CGRect(x: rect.origin.x * image.scale, y: rect.origin.y * image.scale,
                                                           width: rect.width * image.scale, height: rect.height * image.scale)) {
                UIImage(cgImage: cg, scale: image.scale, orientation: image.imageOrientation)
                    .draw(in: CGRect(x: 0, y: 0, width: out, height: out))
            } else {
                image.draw(in: CGRect(x: 0, y: 0, width: out, height: out))
            }
        }
    }
}

/// The picture master every avatar upload sends — the iPhone twin of the relay's
/// `avatar_image::master` and the Mac's `sync::avatar_master`: ≤ 1024 px, alpha over white,
/// **PNG** when the picture is a flat graphic (a mark, a logo: a handful of colours cover almost
/// every pixel, the rest being its anti-aliased edges), **JPEG q92** for a photograph. The relay
/// checks that the `Content-Type` and the bytes agree, so [`mime`] / [`ext`] read the bytes.
enum PictureMaster {
    /// Longest side of the master, in pixels.
    static let side: CGFloat = 1024
    /// The relay's cap for `PUT /me/avatar`.
    static let maxBytes = 3 * 1024 * 1024
    static let jpegQuality: CGFloat = 0.92
    static let flatMaxColors = 64
    static let flatCoverage = 0.90

    /// Encode `image` (already squared / sized by `AvatarPickerSheet.square`).
    static func encode(_ image: UIImage) -> Data? {
        let flattened = overWhite(image)
        if isFlatGraphic(flattened), let png = flattened.pngData(), png.count <= maxBytes {
            return png
        }
        if let jpeg = flattened.jpegData(compressionQuality: jpegQuality), jpeg.count <= maxBytes {
            return jpeg
        }
        return flattened.jpegData(compressionQuality: 0.85)
    }

    static func isPNG(_ data: Data) -> Bool {
        data.starts(with: [0x89, 0x50, 0x4E, 0x47])
    }

    static func mime(_ data: Data) -> String {
        isPNG(data) ? "image/png" : "image/jpeg"
    }

    static func ext(_ data: Data) -> String {
        isPNG(data) ? "png" : "jpg"
    }

    /// Transparent pixels land on white (the routes serve JPEG too, and every disc draws on its
    /// own background — one rule keeps the two formats alike).
    static func overWhite(_ image: UIImage) -> UIImage {
        let format = UIGraphicsImageRendererFormat()
        format.scale = image.scale
        format.opaque = true
        return UIGraphicsImageRenderer(size: image.size, format: format).image { ctx in
            UIColor.white.setFill()
            ctx.fill(CGRect(origin: .zero, size: image.size))
            image.draw(at: .zero)
        }
    }

    /// Sampled on a 128 × 128 grid; colours quantised to 5 bits a channel.
    static func isFlatGraphic(_ image: UIImage) -> Bool {
        guard let cg = image.cgImage else { return false }
        let side = 128
        var pixels = [UInt8](repeating: 0, count: side * side * 4)
        let info = CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
        let drawn = pixels.withUnsafeMutableBytes { buf -> Bool in
            guard let ctx = CGContext(data: buf.baseAddress, width: side, height: side, bitsPerComponent: 8,
                                      bytesPerRow: side * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: info)
            else { return false }
            ctx.interpolationQuality = .high
            ctx.draw(cg, in: CGRect(x: 0, y: 0, width: side, height: side))
            return true
        }
        guard drawn else { return false }
        var counts: [UInt32: Int] = [:]
        for i in stride(from: 0, to: pixels.count, by: 4) {
            let key = UInt32(pixels[i] & 0xF8) << 16 | UInt32(pixels[i + 1] & 0xF8) << 8 | UInt32(pixels[i + 2] & 0xF8)
            counts[key, default: 0] += 1
        }
        let total = side * side
        let covered = counts.values.sorted(by: >).prefix(flatMaxColors).reduce(0, +)
        return Double(covered) / Double(total) >= flatCoverage
    }
}
