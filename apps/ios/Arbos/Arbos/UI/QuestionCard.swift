// The Grok Bot question card, measured from internal/refs/grokbot-question-card-light.jpg
// and media/design-parity/reference/grokbot-ios-dark/chat-question-card-dark.png (@3x):
// one rounded card in the assistant-bubble grey (24 pt radius, 14 pt padding), a semibold
// 17.5 pt question, a thin grey × top-right, an inner list (14 pt radius) of 41 pt rows —
// a 24 pt lettered badge, 17 pt text, hairline separators — and, while open, the muted
// line "Or answer in the chat below". Dark: the list is the page colour (#141414) inset
// into the #202020 card, badges are card-coloured with #5D5D60 letters. No icons, chips
// or coloured buttons.
//
// Tapping a row answers the question (`answer { questionId, value }`); × hides the card
// for this session, the composer still answers. An answered card keeps the picked row
// and fades the others.

import SwiftUI

struct QuestionCard: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme

    let rowId: String
    let chatId: Id
    /// nil for a legacy `waiting_user` status line: title only, the composer answers.
    let questionId: String?
    let title: String
    let options: [ChoiceOption]
    let multi: Bool
    /// Picked option ids once answered.
    let picked: [String]
    let open: Bool

    /// #1544: a destructive-shell card; the footer says a standing choice is kept per agent.
    var shellCard: Bool = false
    /// Secret hand-off request card: Name + masked Value + "Save to vault" instead of rows.
    var offer: SecretOffer? = nil

    @State private var dismissed = false
    @State private var selected: Set<String> = []
    @State private var offerName = ""
    @State private var offerValue = ""
    @State private var offerShow = false
    @State private var offerSeeded = false
    @State private var handoffErrorDetailsOpen = false
    @FocusState private var offerFocus: OfferField?

    private enum OfferField { case name, value }

    /// The agent whose card this is, for "Always allow Doug in /tmp".
    private var agentName: String {
        if let pid = model.chat(chatId)?.projectId, let p = model.project(pid) { return p.name }
        return "the agent"
    }

    private func named(_ s: String) -> String {
        s.replacingOccurrences(of: "{agent}", with: agentName)
    }

    static let radius: CGFloat = 24
    static let pad: CGFloat = 14
    static let listRadius: CGFloat = 14
    static let rowHeight: CGFloat = 41
    static let badge: CGFloat = 24

    var body: some View {
        if dismissed {
            EmptyView()
        } else {
            VStack(alignment: .leading, spacing: 10) {
                header
                if offer != nil {
                    offerBody
                } else if !options.isEmpty {
                    list
                }
                if open && !options.isEmpty {
                    Text(shellCard
                         ? "Standing choices are per agent: \(agentName) stops asking for what you allow here. Or answer in the chat below."
                         : "Or answer in the chat below")
                        .font(theme.font(Theme.small))
                        .foregroundStyle(palette.badgeInk)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.top, 4)
                        .padding(.leading, 1)
                        .accessibilityIdentifier(shellCard ? "shell-card-foot" : "question-card-foot")
                }
            }
            .padding(.top, Self.pad)
            .padding(.horizontal, Self.pad)
            .padding(.bottom, Self.pad + 1)
            .background(RoundedRectangle(cornerRadius: Self.radius, style: .continuous).fill(palette.outer))
        }
    }

    // MARK: Secret hand-off request card

    /// The device's own view of this card's save (saving / saved / failed), keyed by approval id.
    private var handoff: AppModel.HandoffState? {
        guard let questionId, let id = questionId.stripping(prefix: "approval:") else { return nil }
        return model.handoff[id]
    }

    private var offerTitle: String {
        guard let offer else { return title }
        let agent = offer.agent.isEmpty ? agentName : offer.agent
        if case .saved(let env) = handoff { return "\(env) is in your vault" }
        if !open { return picked.contains("saved") ? "\(offer.name) is in your vault" : "Secret not saved" }
        return "Give \(agent) a secret"
    }

    private var offerOpen: Bool {
        if case .saved = handoff { return false }
        return open
    }

    private var offerBody: some View {
        let agent = offer?.agent.isEmpty == false ? offer!.agent : agentName
        let saving: Bool = { if case .saving = handoff { return true }; return false }()
        let env = offerName.trimmingCharacters(in: .whitespaces).uppercased()
            .replacingOccurrences(of: " ", with: "_").replacingOccurrences(of: "-", with: "_")
        let canSave = !offerValue.trimmingCharacters(in: .whitespaces).isEmpty && !env.isEmpty && !saving
        return VStack(alignment: .leading, spacing: 10) {
            VStack(spacing: 0) {
                if offerOpen {
                    offerField(label: "Name", field: .name) {
                        TextField("STRIPE_KEY", text: $offerName)
                            .textInputAutocapitalization(.characters)
                            .autocorrectionDisabled()
                            .submitLabel(.next)
                            .onSubmit { offerFocus = .value }
                            .accessibilityIdentifier("secret-offer-name")
                    }
                    hairline
                    offerField(label: "Value", field: .value) {
                        HStack(spacing: 10) {
                            Group {
                                if offerShow {
                                    TextField("Paste the key", text: $offerValue)
                                } else {
                                    SecureField("Paste the key", text: $offerValue)
                                }
                            }
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .submitLabel(.done)
                            .onSubmit { if canSave { saveOffer() } }
                            .accessibilityIdentifier("secret-offer-value")
                            if !offerValue.isEmpty {
                                Button(offerShow ? "Hide" : "Show") { offerShow.toggle() }
                                    .font(theme.font(Theme.small))
                                    .foregroundStyle(theme.muted)
                                    .buttonStyle(.plain)
                            }
                        }
                    }
                    hairline
                    Button {
                        saveOffer()
                    } label: {
                        HStack {
                            Text(saving ? "Saving…" : "Save to vault")
                                .font(theme.font(Theme.body, weight: .semibold))
                                .foregroundStyle(canSave ? theme.text : theme.placeholder)
                            Spacer(minLength: 0)
                        }
                        .padding(.horizontal, 10)
                        .frame(minHeight: Self.rowHeight)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(RowPress(enabled: canSave, fill: palette.press))
                    .disabled(!canSave)
                    .accessibilityIdentifier("secret-offer-save")
                } else {
                    let saved = picked.contains("saved") || { if case .saved = handoff { return true }; return false }()
                    HStack(spacing: 11) {
                        Text(saved ? "✓" : "–")
                            .font(theme.font(14))
                            .foregroundStyle(saved ? palette.inner : palette.badgeInk)
                            .frame(width: Self.badge, height: Self.badge)
                            .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(saved ? theme.text : palette.badge))
                        Text(saved ? "Saved to your vault" : "Not saved")
                            .font(theme.font(Theme.body))
                            .foregroundStyle(saved ? theme.text : theme.placeholder)
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .frame(minHeight: Self.rowHeight)
                }
            }
            .background(RoundedRectangle(cornerRadius: Self.listRadius, style: .continuous).fill(palette.inner))
            .clipShape(RoundedRectangle(cornerRadius: Self.listRadius, style: .continuous))
            if offerOpen {
                Text("Sent encrypted from this device to your vault. \(agent) gets only the name — never the value.")
                    .font(theme.font(Theme.small))
                    .foregroundStyle(palette.badgeInk)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, 1)
                    .accessibilityIdentifier("secret-offer-foot")
                if case .failed(let error) = handoff {
                    // The card's one note (Jacob 2026-10-26); the value was forgotten on Save, so
                    // there is no Try again — the field above is the retry.
                    ErrorNote(id: "question-card-error", error: error, detailsOpen: $handoffErrorDetailsOpen)
                }
            } else if picked.contains("saved") || { if case .saved = handoff { return true }; return false }() {
                Text("\(agent) can use it. Where it may be used is set the first time it is used. Manage it in Settings → Secrets.")
                    .font(theme.font(Theme.small))
                    .foregroundStyle(theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, 1)
            }
        }
        .onAppear {
            guard !offerSeeded, let offer else { return }
            offerSeeded = true
            offerName = offer.name
            offerFocus = offer.name.isEmpty ? .name : .value
        }
    }

    private func offerField<Content: View>(label: String, field: OfferField, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label)
                .font(theme.font(Theme.tiny))
                .foregroundStyle(theme.muted)
            content()
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.text)
                .focused($offerFocus, equals: field)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .frame(minHeight: Self.rowHeight + 14, alignment: .leading)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(theme.accent, lineWidth: offerFocus == field ? 1.5 : 0)
        )
        .contentShape(Rectangle())
        .onTapGesture { offerFocus = field }
    }

    private func saveOffer() {
        guard let offer, let questionId, let approvalId = questionId.stripping(prefix: "approval:") else { return }
        let value = offerValue
        let env = offerName.trimmingCharacters(in: .whitespaces).uppercased()
            .replacingOccurrences(of: " ", with: "_").replacingOccurrences(of: "-", with: "_")
        guard !value.trimmingCharacters(in: .whitespaces).isEmpty, !env.isEmpty else { return }
        Haptics.pick()
        let projectId = offer.projectId.isEmpty ? (model.chat(chatId)?.projectId ?? "") : offer.projectId
        model.saveSecretFromCard(chatId: chatId, approvalId: approvalId, projectId: projectId,
                                 name: env, value: value, purpose: offer.purpose)
        // The field forgets the value as soon as it has left this view.
        offerValue = ""
        offerShow = false
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 10) {
            Text(offer == nil ? title : offerTitle)
                .font(theme.font(17.5, weight: .semibold))
                .foregroundStyle(theme.text)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.leading, 1)
                .frame(maxWidth: .infinity, alignment: .leading)
            if offer == nil ? open : offerOpen {
                Button {
                    withAnimation(.easeOut(duration: 0.15)) { dismissed = true }
                    // The request card's × declines: the agent learns "not saved", nothing else.
                    if offer != nil, let questionId, let approvalId = questionId.stripping(prefix: "approval:") {
                        model.decideApproval(chatId: chatId, approvalId: approvalId, choice: "reject")
                    }
                } label: {
                    CloseGlyph(color: palette.close)
                        .frame(width: 12, height: 12)
                        .frame(width: 28, height: 22)
                        // 44 pt touch target around the thin ×, no layout change.
                        .contentShape(Rectangle().inset(by: -11))
                }
                .buttonStyle(.plain)
                .padding(.trailing, -4)
                .accessibilityLabel("Dismiss")
            }
        }
    }

    private var list: some View {
        VStack(spacing: 0) {
            ForEach(Array(options.enumerated()), id: \.element.id) { ix, option in
                if ix > 0 { hairline }
                optionRow(ix: ix, option: option)
            }
            if multi && open {
                hairline
                sendRow
            }
        }
        .background(RoundedRectangle(cornerRadius: Self.listRadius, style: .continuous).fill(palette.inner))
        .clipShape(RoundedRectangle(cornerRadius: Self.listRadius, style: .continuous))
    }

    private var hairline: some View {
        Rectangle().fill(palette.hairline).frame(height: theme.isDark ? 1 : 1 / UIScreen.main.scale)
    }

    private func optionRow(ix: Int, option: ChoiceOption) -> some View {
        let chosen = open ? selected.contains(option.id) : picked.contains(option.id)
        let faded = !open && !chosen
        return Button {
            guard open else { return }
            Haptics.pick()
            if multi {
                if selected.remove(option.id) == nil { selected.insert(option.id) }
            } else {
                answer(ids: [option.id], label: named(option.label))
            }
        } label: {
            HStack(spacing: 11) {
                Text(Self.letter(ix))
                    .font(theme.font(14))
                    .foregroundStyle(chosen ? palette.inner : palette.badgeInk)
                    .frame(width: Self.badge, height: Self.badge)
                    .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(chosen ? theme.text : palette.badge))
                Text(named(option.label))
                    .font(theme.font(Theme.body))
                    .foregroundStyle(faded ? theme.placeholder : (option.destructive ? theme.danger : theme.text))
                    .multilineTextAlignment(.leading)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
                if let hint = option.hint, !hint.isEmpty {
                    Text(named(hint))
                        .font(theme.font(Theme.tiny))
                        .foregroundStyle(palette.badgeInk)
                        .multilineTextAlignment(.trailing)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: 150, alignment: .trailing)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .frame(minHeight: Self.rowHeight)
            .contentShape(Rectangle())
        }
        .buttonStyle(RowPress(enabled: open, fill: palette.press))
        .disabled(!open)
        .accessibilityLabel("\(Self.letter(ix)), \(named(option.label))")
        .accessibilityIdentifier("option-\(option.id)")
    }

    private var sendRow: some View {
        Button {
            let labels = options.filter { selected.contains($0.id) }.map(\.label)
            guard !labels.isEmpty else { return }
            answer(ids: options.map(\.id).filter { selected.contains($0) }, label: labels.joined(separator: ", "))
        } label: {
            HStack {
                Text("Send")
                    .font(theme.font(Theme.body, weight: .semibold))
                    .foregroundStyle(selected.isEmpty ? theme.placeholder : theme.text)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 10 + Self.badge + 11)
            .frame(minHeight: Self.rowHeight)
            .contentShape(Rectangle())
        }
        .buttonStyle(RowPress(enabled: !selected.isEmpty, fill: palette.press))
        .disabled(selected.isEmpty)
    }

    private func answer(ids: [String], label: String) {
        guard let questionId else { return }
        let value: Any = multi ? ids : (ids.first ?? "")
        model.answerQuestion(chatId: chatId, questionId: questionId, value: value, label: label)
    }

    static func letter(_ ix: Int) -> String {
        ix < 26 ? String(UnicodeScalar(UInt8(65 + ix))) : String(ix + 1)
    }

    // MARK: Palette

    private struct Palette {
        var outer: Color
        var inner: Color
        var badge: Color
        var badgeInk: Color
        var close: Color
        var hairline: Color
        var press: Color
    }

    private var palette: Palette {
        if theme.isDark {
            // Measured on the real app: list = page #141414, badge = card #202020,
            // letters / × / footnote #5D5D60, separators in the card colour.
            return Palette(outer: theme.bubbleAssistant, inner: theme.bg, badge: theme.bubbleAssistant,
                           badgeInk: Color(hex: "#5D5D60")!, close: Color(hex: "#5D5D60")!,
                           hairline: theme.bubbleAssistant, press: Color.white.opacity(0.06))
        }
        return Palette(outer: theme.bubbleAssistant, inner: .white, badge: Color(hex: "#F1F1F3")!,
                       badgeInk: Color(hex: "#B4B4B8")!, close: Color(hex: "#B4B4B8")!,
                       hairline: Color.black.opacity(0.06), press: Color.black.opacity(0.04))
    }
}

/// Thin × drawn as two strokes (SF's xmark is heavier than Grok's).
private struct CloseGlyph: View {
    let color: Color

    var body: some View {
        Canvas { ctx, size in
            var path = Path()
            path.move(to: CGPoint(x: 0, y: 0))
            path.addLine(to: CGPoint(x: size.width, y: size.height))
            path.move(to: CGPoint(x: size.width, y: 0))
            path.addLine(to: CGPoint(x: 0, y: size.height))
            ctx.stroke(path, with: .color(color), style: StrokeStyle(lineWidth: 1.4, lineCap: .round))
        }
    }
}

/// Row highlight while pressed; no other affordance.
private struct RowPress: ButtonStyle {
    let enabled: Bool
    let fill: Color

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(enabled && configuration.isPressed ? fill : .clear)
    }
}
