// Settings → Agent access link (Jacob, 2026-11-03): the one place the account hands an
// outside agent a way in, and the one place every agent token is listed.
//
// Four cards, in the order the decision is made:
//
//   Scope          two rows with a consequence each; the footer swaps with the choice
//   Expires        four options, 7 days by default; there is no "Never"
//   Create …       the button names the scope, so there is no generic Create to press
//   Active links   every live token, with its scope and a Revoke
//
// Then the copy sheet, which is the only screen the link exists on. The store layer is
// `Store/AgentLinksStore.swift`; the design is `internal/settings-agent-access-link-*.md`
// and the approved mock-ups are `media/agent-access-link-settings-mockup-{3,4}.png`.

import SwiftUI
import UIKit

/// The pushed Settings page.
struct AgentAccessLinkPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    /// Rig: `ARBOS_SETTINGS_PAGE=agent-links` lands here; `agent-links-copy` lands with the
    /// copy sheet up, and `agent-links-copied` with its Copy already pressed.
    var landing: String? = DebugHooks.settingsPage
    /// Read only is the default and reply-as-me is never pre-selected.
    @State private var scope: AgentLinkScope = .read
    @State private var expiry: AgentLinkExpiry = .standard
    @State private var label = ""
    @State private var creating = false
    @State private var reveal: AgentLinkReveal?
    @State private var errorDetailsOpen = false

    private var rows: [AgentAccessLink] { model.agentLinkRows }

    var body: some View {
        SettingsPage(title: "Agent access link") {
            if let why = model.agentLinksError {
                ErrorNote(id: "agent-links-error", error: why, detailsOpen: $errorDetailsOpen,
                          onTryAgain: { Task { await model.refreshAgentLinks() } })
            }
            scopeCard
            expiryCard
            if model.agentLinksMintable { createCard }
            listCard
        }
        .task {
            await model.refreshAgentLinks()
            guard let landing, landing.hasPrefix("agent-links-cop") else { return }
            scope = .reply
            // Rig URL: a short open-secret tail only — never a real bearer.
            reveal = AgentLinkReveal(id: "lnk-rig", label: "Jacob's Claude", scope: .reply,
                                     expiry: .standard,
                                     url: "https://relay.arbos.life/agent-link/open/fxOpen1")
        }
        .sheet(item: $reveal) { made in
            AgentLinkCopySheet(reveal: made, startCopied: landing == "agent-links-copied") { reveal = nil }
                .environmentObject(model)
                .environment(\.theme, theme)
        }
    }

    // MARK: Scope

    /// Two tall rows against expiry's one compact pill: the safe, reversible setting gets the
    /// small control and the dangerous, irreversible one gets the room.
    private var scopeCard: some View {
        PaneCard(header: "Scope", footer: scope.footer, separatorInset: PaneKit.inset) {
            ForEach(AgentLinkScope.allCases) { choice in
                scopeRow(choice)
            }
        }
    }

    private func scopeRow(_ choice: AgentLinkScope) -> some View {
        // Danger ink appears only while the dangerous option is chosen, so red means exactly
        // one thing on this screen: this can speak as you.
        let chosen = choice == scope
        let loud = chosen && choice.isDangerous
        return Button { scope = choice } label: {
            HStack(spacing: PaneKit.gap) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(choice.label)
                        .font(theme.font(Theme.body))
                        .foregroundStyle(loud ? theme.danger : theme.text)
                    Text(choice.consequence)
                        .font(theme.font(Theme.small))
                        .foregroundStyle(loud ? theme.danger : theme.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 8)
                if chosen {
                    Image(systemName: "checkmark")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(loud ? theme.danger : theme.text)
                }
            }
            .padding(.horizontal, PaneKit.inset)
            .padding(.vertical, 8)
            .frame(minHeight: 62)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("agent-link-scope-\(choice.rawValue)")
        .accessibilityAddTraits(chosen ? [.isSelected] : [])
    }

    // MARK: Expires

    private var expiryCard: some View {
        PaneCard(header: "Expires",
                 footer: "Every link expires. Revoke it sooner when the agent is done.",
                 separatorInset: PaneKit.inset) {
            Picker("Expires", selection: $expiry) {
                ForEach(AgentLinkExpiry.allCases) { option in
                    Text(option.label).tag(option)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(.horizontal, PaneKit.inset)
            .padding(.vertical, 10)
            .frame(minHeight: PaneKit.rowHeight)
            .accessibilityIdentifier("agent-link-expiry")
        }
    }

    // MARK: Create

    private var createCard: some View {
        let blankDefault = AgentAccessLink.nextLabel(model.agentLinks)
        return PaneCard(
            footer: "Name it for the agent you are giving it to. Left blank it is called “\(blankDefault)”. One link is all you hand over, shown on the next screen, once.",
            separatorInset: PaneKit.inset
        ) {
            nameRow
            SettingsActionRow(title: scope.createAction, destructive: scope.isDangerous, disabled: creating) {
                create()
            }
            .accessibilityIdentifier("agent-link-create")
        }
    }

    /// The link's name, which is what its row in the list below is called. Left empty, the
    /// relay names every link "Agent link", and a list of those cannot be read.
    private var nameRow: some View {
        HStack(spacing: 12) {
            Text("Name").font(theme.font(Theme.body)).foregroundStyle(theme.text)
            TextField("", text: $label, prompt: Text("Jacob's Claude").foregroundStyle(theme.placeholder))
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.text)
                .multilineTextAlignment(.trailing)
                .textInputAutocapitalization(.words)
                .autocorrectionDisabled()
                .submitLabel(.done)
                .accessibilityIdentifier("agent-link-name")
        }
        .padding(.horizontal, PaneKit.inset)
        .frame(minHeight: SettingsRow.height)
    }

    private func create() {
        guard !creating else { return }
        creating = true
        Task {
            let made = await model.createAgentLink(label: label, scope: scope, expiry: expiry)
            creating = false
            guard let made else { return }
            label = ""
            reveal = made
        }
    }

    // MARK: Active links

    @ViewBuilder private var listCard: some View {
        if !rows.isEmpty {
            SettingsGroup(header: "Active links", footer: listFooter) {
                ForEach(rows) { link in
                    linkRow(link)
                }
            }
        } else if !model.agentLinksLoaded {
            Text("Loading…").font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                .frame(maxWidth: .infinity).padding(.top, 36)
        } else {
            Text("No links yet").font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                .frame(maxWidth: .infinity).padding(.top, 36)
        }
    }

    /// The relay's own sentence about a row that is honestly unlike the others (an export token
    /// keeps no activity record) joins the footnote rather than crowding the row.
    private var listFooter: String {
        let base = "Revoking stops a link at once. What an agent already read stays read."
        guard let note = rows.compactMap(\.note).first else { return base }
        return "\(base) \(note)"
    }

    /// The tag sits beside the name, not at the trailing edge: whether a live link can speak as
    /// you is the single most important fact about it, so it reads with the name.
    private func linkRow(_ link: AgentAccessLink) -> some View {
        HStack(spacing: PaneKit.gap) {
            PaneTile(icon: "link")
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 7) {
                    Text(link.displayName)
                        .font(theme.font(Theme.body))
                        .foregroundStyle(theme.text)
                        .lineLimit(1)
                    tag(link.scope)
                }
                Text(link.subtitle())
                    .font(theme.font(Theme.small))
                    .foregroundStyle(theme.muted)
                    .lineLimit(1)
            }
            .layoutPriority(1)
            Spacer(minLength: 8)
            // Revoke acts at once, as the Mac does: the link is already out of the owner's
            // hands, so a confirm step only keeps it alive longer.
            Button { model.revokeAgentLink(link.id) } label: {
                // error-ui: exception destructive-action
                Text("Revoke").font(theme.font(Theme.small)).foregroundStyle(theme.danger)
                    .lineLimit(1).fixedSize()
            }
            .buttonStyle(.borderless)
            .accessibilityLabel("Revoke \(link.displayName)")
            .accessibilityIdentifier("agent-link-revoke-\(link.id)")
        }
        .padding(.horizontal, PaneKit.inset)
        .padding(.vertical, 8)
        .frame(minHeight: 64)
        .accessibilityIdentifier("agent-link-row-\(link.id)")
    }

    /// The Devices page's kind-tag capsule, in danger ink for a link that replies as you.
    private func tag(_ scope: AgentLinkScope) -> some View {
        Text(scope.tag)
            .font(theme.font(Theme.tiny))
            .foregroundStyle(scope.isDangerous ? theme.danger : theme.muted)
            .lineLimit(1).fixedSize()
            .padding(.horizontal, 6).padding(.vertical, 2)
            // control-size: exception inlineChips
            .background(Capsule().fill(scope.isDangerous ? theme.dangerSoft : theme.field))
    }
}

/// The copy sheet, shown once. It is the only screen the URL exists on, which sets every rule
/// here: a fitted height so there is no empty page under it, no drag indicator and no swipe-down
/// before the copy, and a Done that waits for Copy. The × is always a way out.
///
/// The geometry is `ConnectRevealView`'s, to the point. `AvatarSheetPalette` is a fixed dark
/// palette, so this sheet does not follow the Glass / Matte setting — neither does any other
/// sheet in that family, and changing that is work inside the shared component.
struct AgentLinkCopySheet: View {
    @Environment(\.theme) private var theme
    let reveal: AgentLinkReveal
    /// Rig only (`ARBOS_SETTINGS_PAGE=agent-links-copied`): land on the post-copy state, which
    /// is what the approved mock-up shows.
    var startCopied = false
    let onDone: () -> Void
    @State private var copied = false

    private let sheetBg = AvatarSheetPalette.sheetBg
    private let line = AvatarSheetPalette.line
    private let placeholder = AvatarSheetPalette.placeholder
    private let cardBg = AvatarSheetPalette.pillBg
    private let tileBg = AvatarSheetPalette.ring
    private let ink = AvatarSheetPalette.title

    /// The content's own height, so the sheet is fitted: header 54.5, two section titles at 46,
    /// two 60 pt cards, footnotes of two and three lines, and the 104 pt footer.
    static let height: CGFloat = 54.5 + 46 + 60 + 46 + 65 + 46 + 60 + 4 + 104

    var body: some View {
        VStack(spacing: 0) {
            header
            ScrollView(showsIndicators: false) {
                VStack(alignment: .leading, spacing: 0) {
                    sectionTitle("Give this to your agent")
                    card { copyRow }
                    footnote("This link opens once. Give it to your agent now — you will not see this URL again.")
                    footnote("Whatever opens it first gets the access, and your agent then sees an error. If that happens, revoke this link and make a new one.")
                    sectionTitle("Scope")
                    card { scopeRow }
                }
                .padding(.bottom, 4)
            }
            footer
        }
        .background(sheetBg.ignoresSafeArea())
        .onAppear { copied = startCopied }
        .presentationDetents([.height(Self.height)])
        .presentationDragIndicator(.hidden)
        .presentationBackground(sheetBg)
        .interactiveDismissDisabled(!copied)
    }

    private var header: some View {
        HStack {
            Text("New agent access link")
                .font(.system(size: 14.2, weight: .semibold))
                .foregroundStyle(ink)
                .lineLimit(1)
                .padding(.leading, 17.5)
            Spacer()
            Button(action: onDone) {
                Image(systemName: "xmark")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(Color(white: 0xD2 / 255))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .padding(.trailing, 5.5)
            .accessibilityLabel("Close")
        }
        .frame(height: 54.5)
        .overlay(alignment: .bottom) { Rectangle().fill(line).frame(height: 1) }
    }

    /// No leading tile. In the precedent the tile told the Link row from the Key row; with one
    /// row it identifies nothing, and its 50 pt was why the URL truncated.
    ///
    /// The owner copies or shares the open URL — that is what hands the agent its `llms.txt`
    /// (with the bearer inside). The owner's device never holds the bearer itself.
    private var copyRow: some View {
        HStack(spacing: 14) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Link").font(.system(size: 13)).foregroundStyle(placeholder)
                Text(AgentLinkReveal.shortUrl(reveal.url))
                    .font(.system(size: 15, design: .monospaced))
                    .foregroundStyle(ink)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            if let url = URL(string: reveal.url) {
                ShareLink(item: url) {
                    capsule(glyph: "square.and.arrow.up", title: "Share", on: false)
                }
                .simultaneousGesture(TapGesture().onEnded { copied = true })
                .accessibilityIdentifier("agent-link-share")
            }
            Button {
                UIPasteboard.general.string = reveal.url
                copied = true
            } label: {
                capsule(glyph: copied ? "checkmark" : "doc.on.doc",
                        title: copied ? "Copied" : "Copy",
                        on: copied)
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("agent-link-copy")
        }
        .padding(.horizontal, 16)
        .frame(minHeight: 60)
    }

    private func capsule(glyph: String, title: String, on: Bool) -> some View {
        HStack(spacing: 6) {
            Image(systemName: glyph)
                .font(.system(size: 13, weight: .medium))
            Text(title)
                .font(.system(size: 14, weight: .medium))
        }
        .foregroundStyle(on ? theme.online : ink)
        .padding(.horizontal, 12)
        .frame(height: 32)
        .background(Capsule().fill(on ? theme.online.opacity(0.18) : tileBg))
    }

    /// The scope again, with its expiry: you should not be able to copy a reply-as-me link
    /// believing it is read only. This tile is not an identifier — its fill carries the warning.
    private var scopeRow: some View {
        HStack(spacing: 14) {
            Image(systemName: reveal.scope.glyph)
                .font(.system(size: 17, weight: .medium))
                .foregroundStyle(reveal.scope.isDangerous ? theme.danger : ink)
                .frame(width: 36, height: 36)
                .background(
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .fill(reveal.scope.isDangerous ? theme.dangerSoft : tileBg)
                )
            VStack(alignment: .leading, spacing: 2) {
                Text(reveal.scope.label)
                    .font(.system(size: 16))
                    .foregroundStyle(reveal.scope.isDangerous ? theme.danger : ink)
                Text(reveal.scope.restatement(reveal.expiry))
                    .font(.system(size: 13))
                    .foregroundStyle(placeholder)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
        }
        .padding(.horizontal, 16)
        .frame(minHeight: 60)
    }

    /// Done in `create_entity`'s own disabled tokens until Copy has been pressed: a closed sheet
    /// is unrecoverable, so the way out should not be the easiest thing to hit.
    private var footer: some View {
        HStack(spacing: 10) {
            Spacer()
            Button(action: onDone) {
                Text("Done")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(copied ? theme.onPrimary : AvatarSheetPalette.disabledText)
                    .frame(width: 92, height: 44)
                    .background(Capsule().fill(copied ? theme.primary : AvatarSheetPalette.disabledBg))
            }
            .buttonStyle(.plain)
            .disabled(!copied)
            .accessibilityIdentifier("agent-link-done")
        }
        .padding(.trailing, 16)
        .padding(.top, 25)
        .padding(.bottom, 35)
        .overlay(alignment: .top) { Rectangle().fill(line).frame(height: 1) }
    }

    private func sectionTitle(_ text: String) -> some View {
        Text(text.uppercased())
            .font(.system(size: 13))
            .foregroundStyle(placeholder)
            .padding(.horizontal, 32)
            .padding(.top, 22)
            .padding(.bottom, 8)
    }

    private func footnote(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 13))
            .foregroundStyle(placeholder)
            .lineSpacing(3)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.horizontal, 32)
            .padding(.top, 8)
    }

    private func card<Rows: View>(@ViewBuilder _ rows: () -> Rows) -> some View {
        VStack(spacing: 0) { rows() }
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(cardBg))
            .padding(.horizontal, 16)
    }
}
