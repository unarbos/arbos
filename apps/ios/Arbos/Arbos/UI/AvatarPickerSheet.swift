// The avatar picker sheet (D92 amendment, Jacob 2026-10-23): Grok Bot's avatar picker for an
// agent's picture — a tab pill Bot | Generate | Upload with Reset at the right, a body per tab,
// and a footer Cancel | Set avatar. Nothing changes until "Set avatar".
//
// Bot: the sixteen D92 (v2) leaves as their 4 × 4 sheet, every leaf neutral and the selected one
// in its hue (pastel disc, darker leaf); the ten hues in two rows of five. The per-id
// automatic default stays: Reset goes back to it. Generate: a short prompt → the relay paints one
// picture (`POST /projects/{id}/avatar/generate`) → previewed here; Set keeps it. Upload: Photos
// (images only), uploaded as a 512 px JPEG on Set. Groups and servers open the same sheet with
// the same three tabs (their own `/avatar/generate` route and blob slot).
//
// Grok's sizes from the 2x reference (612 × 794 px → 306 × 397 pt), inside the shipped
// bottom-sheet frame: a 44 pt bar with a hairline, the Create sheet's 33 / 40 / 56 leaf grid,
// 23 pt swatches on a 40 pt pitch, two equal footer pills 8 pt apart (44 pt tall here, the
// touch minimum — the same deviation as the Create sheet).

import ArbosFFI
import PhotosUI
import SwiftUI
import UIKit

/// Whose picture the sheet edits.
enum AvatarTarget: Equatable {
    case agent(Id)
    case group(String)
    case server(String)
}

/// Which body is up.
enum AvatarPickerTab: String, CaseIterable, Identifiable {
    var id: String { rawValue }
    case bot = "Bot"
    case generate = "Generate"
    case upload = "Upload"
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
    @State private var prompt = ""
    @State private var generating = false
    @State private var generatedKey: String?
    @State private var errorText: String?
    @State private var photoItem: PhotosPickerItem?
    @State private var uploadPreview: UIImage?
    @State private var uploadJpeg: Data?
    @State private var saving = false
    @FocusState private var promptFocused: Bool

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

    /// The tabs every target gets — agents, groups and servers alike (Jacob, 2026-10-24: the
    /// group / server picker had lost Generate); Reset sits beside them in the bar.
    static func tabs(for target: AvatarTarget) -> [AvatarPickerTab] {
        switch target {
        case .agent, .group, .server: return [.bot, .generate, .upload]
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
                switch tab {
                case .bot: botBody
                case .generate: generateBody
                case .upload: uploadBody
                }
            }
            .scrollDismissesKeyboard(.interactively)
            footer
        }
        .background(sheetBg.ignoresSafeArea())
        .presentationDetents([.height(397 + 44)])
        .presentationDragIndicator(.visible)
        .onAppear { tab = tabs.contains(initialTab) ? initialTab : .bot }
        .onChange(of: photoItem) { _, item in
            guard let item else { return }
            Task {
                guard let data = try? await item.loadTransferable(type: Data.self),
                      let img = UIImage(data: data) else { return }
                let square = Self.square(img, side: 512)
                uploadPreview = square
                uploadJpeg = square.jpegData(compressionQuality: 0.85)
                reset = false
                errorText = nil
            }
        }
    }

    // MARK: bar — Bot | Generate | Upload …… Reset

    private var bar: some View {
        HStack(spacing: 2) {
            ForEach(tabs, id: \.self) { t in
                Button {
                    tab = t
                    errorText = nil
                } label: {
                    Text(t.rawValue)
                        .font(.system(size: 13, weight: .medium))
                        .foregroundStyle(tab == t ? title : labelMuted)
                        .padding(.horizontal, 11)
                        .frame(height: 28)
                        .background(Capsule().fill(tab == t ? pillBg : .clear))
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(t.rawValue + (tab == t ? ", selected" : ""))
            }
            Spacer()
            Button {
                reset = true
                pickedLeaf = nil
                pickedColor = nil
                uploadJpeg = nil
                uploadPreview = nil
                generatedKey = nil
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

    private var generateBody: some View {
        VStack(spacing: 0) {
            TextField("", text: $prompt, prompt: Text("A fox reading by a lamp").foregroundStyle(placeholder))
                .font(.system(size: 14))
                .foregroundStyle(title)
                .autocorrectionDisabled()
                .focused($promptFocused)
                .submitLabel(.go)
                .onSubmit(generate)
                .padding(.horizontal, 12)
                .frame(height: 35)
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(line, lineWidth: 1))
                .padding(.horizontal, 16.5)
                .padding(.top, 16)
            Button(action: generate) {
                Text(generating ? "Generating…" : (generatedKey == nil ? "Generate" : "Generate again"))
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(canGenerate ? title : disabledText)
                    .padding(.horizontal, 18)
                    .frame(height: 28)
                    .background(Capsule().fill(canGenerate ? pillBg : disabledBg))
            }
            .buttonStyle(.plain)
            .disabled(!canGenerate)
            .padding(.top, 12)
            preview.padding(.top, 16)
            Text(statusLine)
                .font(.system(size: 12))
                .foregroundStyle(errorText == nil ? placeholder : theme.danger)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 16.5)
                .padding(.top, 14)
        }
        .frame(maxWidth: .infinity)
        .padding(.bottom, 24)
    }

    private var canGenerate: Bool { !prompt.trimmingCharacters(in: .whitespaces).isEmpty && !generating }
    private var statusLine: String {
        if let errorText { return errorText }
        if generating { return "Painting a picture — a few seconds." }
        if generatedKey != nil { return "Set avatar keeps this one." }
        return "A few words about the picture you want."
    }

    private func generate() {
        guard canGenerate else { return }
        let text = prompt.trimmingCharacters(in: .whitespaces)
        generating = true
        errorText = nil
        reset = false
        promptFocused = false
        Task {
            do {
                switch target {
                case .agent(let id): generatedKey = try await model.generateProjectAvatar(projectId: id, prompt: text)
                case .group(let id): generatedKey = try await model.generateGroupAvatar(chatId: id, prompt: text)
                case .server(let id): generatedKey = try await model.generateServerAvatar(id, prompt: text)
                }
            } catch {
                errorText = error.localizedDescription
            }
            generating = false
        }
    }

    // MARK: Upload

    private var uploadBody: some View {
        VStack(spacing: 0) {
            preview.padding(.top, 24)
            PhotosPicker(selection: $photoItem, matching: .images) {
                Text(uploadJpeg == nil ? "Choose a photo…" : "Choose another…")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(title)
                    .padding(.horizontal, 18)
                    .frame(height: 28)
                    .background(Capsule().fill(pillBg))
            }
            .buttonStyle(.plain)
            .padding(.top, 16)
            Text(uploadJpeg == nil ? "A picture from Photos; kept at 512 px." : "Set avatar uploads it.")
                .font(.system(size: 12))
                .foregroundStyle(placeholder)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 16.5)
                .padding(.top, 14)
        }
        .frame(maxWidth: .infinity)
        .padding(.bottom, 24)
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
                            // The fixture's "generated" key has no blob behind it: show the leaf.
                            LeafAvatar(pick: currentPick, size: 114)
                        } else {
                            ProgressView().tint(placeholder)
                        }
                    }
                }
            }
            .frame(width: 114, height: 114)
            .clipShape(Circle())
        } else if reset {
            LeafAvatar(pick: currentPick, size: 114)
        } else {
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
            }
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
            }
            saving = false
            if ok { dismiss() }
        }
    }

    private static func square(_ image: UIImage, side: CGFloat) -> UIImage {
        let s = min(image.size.width, image.size.height)
        let rect = CGRect(x: (image.size.width - s) / 2, y: (image.size.height - s) / 2, width: s, height: s)
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: side, height: side))
        return renderer.image { _ in
            if let cg = image.cgImage?.cropping(to: CGRect(x: rect.origin.x * image.scale, y: rect.origin.y * image.scale,
                                                           width: rect.width * image.scale, height: rect.height * image.scale)) {
                UIImage(cgImage: cg, scale: image.scale, orientation: image.imageOrientation)
                    .draw(in: CGRect(x: 0, y: 0, width: side, height: side))
            } else {
                image.draw(in: CGRect(x: 0, y: 0, width: side, height: side))
            }
        }
    }
}
