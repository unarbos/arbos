// "Send feedback" (Grok Bot styled): the captured screen as a card you can
// remove or swap for a photo, a note, Send. Opened by shake, screenshot,
// or Settings. Posts to the relay's feedback endpoint (the fix loop).

import SwiftUI
import PhotosUI

struct FeedbackSheet: View {
    @EnvironmentObject private var model: AppModel
    @ObservedObject private var center = FeedbackCenter.shared
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @State private var note = ""
    @State private var photoItem: PhotosPickerItem?
    @State private var sending = false
    @State private var sent: String?
    @State private var error: String?
    @State private var showFull = false
    @FocusState private var noteFocused: Bool

    private var project: Project? { center.context.projectId.flatMap { model.project($0) } }
    private var canSend: Bool { !sending && (center.screenshot != nil || !note.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty) }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 16) {
                    if let sent {
                        sentView(sent)
                    } else {
                        if let project {
                            HStack(spacing: 8) {
                                AgentAvatar(project: project, theme: theme, size: 22, still: true)
                                Text("About \(project.name)").font(theme.font(Theme.tiny, weight: .medium)).foregroundStyle(theme.muted)
                            }
                            .padding(.horizontal, 12).frame(height: 30)
                            .background(Capsule().fill(theme.field)) // control-size: exception inlineChips
                        }
                        screenshotCard
                        noteField
                        Text("Your screenshot and note go to the Arbos team, with the app version and device. An agent on our side reads every one and fixes what it can.")
                            .font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                            .multilineTextAlignment(.center)
                            .padding(.horizontal, 28)
                        if let error {
                            Text(error).font(theme.font(Theme.tiny)).foregroundStyle(theme.danger).multilineTextAlignment(.center).padding(.horizontal, 24)
                        }
                        Button(action: send) {
                            HStack(spacing: 8) {
                                if sending { ProgressView().tint(theme.onPrimary).controlSize(.small) }
                                Text(sending ? "Sending…" : "Send feedback")
                            }
                            .font(theme.font(Theme.body, weight: .semibold)).foregroundStyle(theme.onPrimary)
                            .frame(maxWidth: .infinity).frame(height: 50)
                            .background(Capsule().fill(theme.primary))
                        }
                        .buttonStyle(.plain)
                        .disabled(!canSend)
                        .opacity(canSend ? 1 : 0.45)
                        .padding(.horizontal, 16)
                        Text("\(FeedbackCenter.appVersion) · \(FeedbackCenter.deviceLine)")
                            .font(theme.font(11)).foregroundStyle(theme.placeholder)
                    }
                    Color.clear.frame(height: 20)
                }
                .padding(.top, 10)
            }
            .scrollDismissesKeyboard(.interactively)
            .background(theme.bg)
            .navigationTitle("Send feedback")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(theme.bg, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { Button(sent == nil ? "Cancel" : "Done") { dismiss() } }
            }
            .onAppear {
                #if DEBUG
                if let text = DebugHooks.autoFeedback, sent == nil, !sending {
                    note = text
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { send() }
                }
                #endif
            }
            .onChange(of: photoItem) { _, item in
                guard let item else { return }
                Task {
                    if let data = try? await item.loadTransferable(type: Data.self), let img = UIImage(data: data) {
                        center.screenshot = img
                    }
                    photoItem = nil
                }
            }
            .fullScreenCover(isPresented: $showFull) {
                if let img = center.screenshot {
                    ZStack(alignment: .topTrailing) {
                        Color.black.ignoresSafeArea()
                        Image(uiImage: img).resizable().scaledToFit().ignoresSafeArea()
                        Button { showFull = false } label: {
                            GlassCircle { Image(systemName: "xmark").font(.system(size: 15, weight: .semibold)).foregroundStyle(.white) }
                        }
                        .buttonStyle(.plain).padding(16)
                    }
                }
            }
        }
        .tint(theme.text)
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .presentationBackground(theme.bg)
        .presentationCornerRadius(28)
        .preferredColorScheme(theme.isDark ? .dark : .light)
    }

    // MARK: Pieces

    private var screenshotCard: some View {
        VStack(spacing: 10) {
            if let img = center.screenshot {
                Button { showFull = true } label: {
                    Image(uiImage: img)
                        .resizable()
                        .scaledToFit()
                        .frame(height: 240)
                        .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(theme.fieldBorder, lineWidth: 1)) // control-size: exception textAreas
                        .shadow(color: .black.opacity(0.35), radius: 14, y: 6)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Screenshot, tap to view")
                HStack(spacing: 10) {
                    PhotosPicker(selection: $photoItem, matching: .images) {
                        Label("Choose photo", systemImage: "photo").font(.system(size: Theme.tiny, weight: .medium))
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(theme.text)
                    .padding(.horizontal, 12).frame(height: 32)
                    .background(Capsule().fill(theme.field)) // control-size: exception inlineChips
                    Button { center.screenshot = nil } label: {
                        Label("Remove", systemImage: "trash").font(.system(size: Theme.tiny, weight: .medium))
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(theme.danger)
                    .padding(.horizontal, 12).frame(height: 32)
                    .background(Capsule().fill(theme.dangerSoft))
                }
            } else {
                PhotosPicker(selection: $photoItem, matching: .images) {
                    VStack(spacing: 8) {
                        Image(systemName: "photo.badge.plus").font(.system(size: 26, weight: .medium)).foregroundStyle(theme.muted)
                        Text("Add a screenshot").font(theme.font(Theme.small, weight: .medium)).foregroundStyle(theme.text)
                        Text("Optional — pick one from Photos").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                    }
                    .frame(maxWidth: .infinity).frame(height: 140)
                    .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(theme.card))
                    .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(theme.fieldBorder, style: StrokeStyle(lineWidth: 1, dash: [6, 5]))) // control-size: exception textAreas
                }
                .buttonStyle(.plain)
                .padding(.horizontal, 16)
            }
        }
    }

    private var noteField: some View {
        ZStack(alignment: .topLeading) {
            if note.isEmpty {
                Text("What happened? What did you expect?")
                    .font(theme.font(Theme.body)).foregroundStyle(theme.placeholder)
                    .padding(.horizontal, 18).padding(.vertical, 14)
            }
            TextEditor(text: $note)
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.text)
                .scrollContentBackground(.hidden)
                .focused($noteFocused)
                .frame(minHeight: 110, maxHeight: 180)
                .padding(.horizontal, 14).padding(.vertical, 6)
        }
        .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(theme.field)) // control-size: exception textAreas
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(theme.fieldBorder, lineWidth: 1)) // control-size: exception textAreas
        .padding(.horizontal, 16)
    }

    private func sentView(_ id: String) -> some View {
        VStack(spacing: 14) {
            GlassCircle(size: 72) { // control-size: exception illustrationDiscs
                Image(systemName: "checkmark").font(.system(size: 30, weight: .semibold)).foregroundStyle(theme.online)
            }
            .padding(.top, 40)
            Text("Thanks — sent.").font(theme.font(22, weight: .semibold)).foregroundStyle(theme.text)
            Text("It is in the fix loop. Reference \(id.suffix(8)).")
                .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                .multilineTextAlignment(.center).padding(.horizontal, 32)
            Button { dismiss() } label: {
                Text("Done").font(theme.font(Theme.body, weight: .semibold)).foregroundStyle(theme.onPrimary)
                    .frame(maxWidth: .infinity).frame(height: 50)
                    .background(Capsule().fill(theme.primary))
            }
            .buttonStyle(.plain).padding(.horizontal, 16).padding(.top, 10)
        }
    }

    // MARK: Send

    private func send() {
        guard canSend else { return }
        sending = true
        error = nil
        noteFocused = false
        let text = note.trimmingCharacters(in: .whitespacesAndNewlines)
        let image = center.screenshot
        let projectId = center.context.projectId
        Task {
            do {
                let id = try await model.sendFeedback(note: text, image: image, projectId: projectId)
                sent = id ?? "ok"
            } catch {
                let e = error as? RelayError
                self.error = (e?.status == 404 || e?.status == 405)
                    ? "This relay cannot take feedback yet. Try again after the next relay update."
                    : "Could not send: \(error.localizedDescription)"
            }
            sending = false
        }
    }
}
