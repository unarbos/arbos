import SwiftUI

/// The one error element (Jacob, 2026-10-26, [feedback error-messages-ios-2026-10-26]; the Mac
/// twin is `ui/error_note.rs`): a soft red block, a red "!" disc, **one plain sentence**, then
/// the actions — **Try again** (or **Sign in**) as a small capsule and **Details**, a disclosure
/// that unfolds a mono block (what · call · where / when / build) with a **Copy** capsule for a
/// feedback report. Every failure the user sees on the iPhone is a `UserError` drawn through
/// this view; nothing else draws red failure text (`ErrorUILintTests` pins it).
///
/// Placement is the caller's, by the rule in `docs/mac-design-rules.md` § "Error messages":
/// sheets → the top of the body under the header / tab pill; settings pages → under the page
/// header; composer failures → directly above the composer; chat-level → pinned under the
/// header pill; toasts → the top of the screen under the status bar (`ToastHost`). The note
/// takes space everywhere but the toast layer; it is never laid over other controls.
///
/// Tokens (`Theme.swift`): fill `dangerSoft`, radius 16, padding 12 / 14, disc 18 in `danger`
/// with a white "!", sentence `Theme.small` 15 / 20 in `text`, Try again a 32 pt capsule (15
/// semibold), Details 15 `muted` + chevron, the Details block `codeChip` radius 12 mono 12 / 17
/// `muted` with a 26 pt Copy capsule; a toast adds a shadow and a × top-right.
struct ErrorNote: View {
    enum Style { case inline, toast }

    @Environment(\.theme) private var theme
    @Environment(\.colorScheme) private var scheme

    /// Prefix of the accessibility identifiers: `<id>`, `<id>-try-again`, `<id>-details`,
    /// `<id>-details-block`, `<id>-copy`, `<id>-close`. One id per surface (see the lint).
    let id: String
    let error: UserError
    @Binding var detailsOpen: Bool
    var style: Style = .inline
    var onTryAgain: (() -> Void)? = nil
    var onSignIn: (() -> Void)? = nil
    var onDismiss: (() -> Void)? = nil

    static let radius: CGFloat = 16
    static let padV: CGFloat = 12
    static let padH: CGFloat = 14
    static let disc: CGFloat = 18
    static let line: CGFloat = 20
    static let pillHeight: CGFloat = 32
    static let detailText: CGFloat = 12
    static let detailLine: CGFloat = 17
    static let copyHeight: CGFloat = 26
    /// The gap a surface leaves between the note and what follows it.
    static let gap: CGFloat = 12

    private var pillFill: Color {
        scheme == .dark ? Color(hex: "#2A2A2C")! : Color(hex: "#E8E8E8")!
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // The sentence row: the actions sit at the end of the line when the sentence fits
            // on one, else right-aligned on a row under it.
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .center, spacing: 14) {
                    sentenceRow.fixedSize(horizontal: true, vertical: false)
                    Spacer(minLength: 0)
                    actions
                }
                VStack(alignment: .leading, spacing: 10) {
                    sentenceRow
                    HStack { Spacer(minLength: 0); actions }
                }
            }
            if detailsOpen {
                detailsBlock
                    .padding(.top, 12)
                    .padding(.leading, Self.disc + 10)
            }
        }
        .padding(.vertical, Self.padV)
        .padding(.leading, Self.padH)
        .padding(.trailing, style == .toast ? Self.padH + 22 : Self.padH)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Self.radius, style: .continuous).fill(theme.dangerSoft))
        .overlay(alignment: .topTrailing) {
            if style == .toast, let onDismiss {
                Button(action: onDismiss) {
                    Image(systemName: "xmark")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(theme.muted)
                        .frame(width: 32, height: 32)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .padding(4)
                .accessibilityIdentifier("\(id)-close")
            }
        }
        .shadow(color: style == .toast ? Color.black.opacity(scheme == .dark ? 0.45 : 0.14) : .clear, radius: 14, y: 6)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(id)
    }

    private var sentenceRow: some View {
        HStack(alignment: .top, spacing: 10) {
            disc.padding(.top, (Self.line - Self.disc) / 2)
            Text(error.sentence)
                .font(theme.font(Theme.small))
                .lineSpacing(Self.line - Theme.small - 3)
                .foregroundStyle(theme.text)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityIdentifier("\(id)-sentence")
        }
    }

    /// The red disc with the white "!" — the only red in the note.
    private var disc: some View {
        ZStack {
            Circle().fill(theme.danger)
            Text("!")
                .font(.system(size: 12.5, weight: .bold))
                .foregroundStyle(.white)
        }
        .frame(width: Self.disc, height: Self.disc)
    }

    private var actions: some View {
        HStack(spacing: 12) {
            switch error.action {
            case .tryAgain:
                if let onTryAgain { capsule("Try again", id: "\(id)-try-again", action: onTryAgain) }
            case .signIn:
                if let onSignIn { capsule("Sign in", id: "\(id)-sign-in", action: onSignIn) }
            case .none:
                EmptyView()
            }
            Button {
                withAnimation(.easeOut(duration: 0.15)) { detailsOpen.toggle() }
            } label: {
                HStack(spacing: 3) {
                    Text("Details")
                    Image(systemName: detailsOpen ? "chevron.up" : "chevron.down")
                        .font(.system(size: 11, weight: .semibold))
                }
                .font(theme.font(Theme.small))
                .foregroundStyle(theme.muted)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("\(id)-details")
        }
        .fixedSize()
    }

    private func capsule(_ label: String, id: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(theme.font(Theme.small, weight: .semibold))
                .foregroundStyle(theme.text)
                .padding(.horizontal, 14)
                .frame(height: Self.pillHeight)
                .background(Capsule().fill(pillFill)) // control-size: exception inlineChips
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(id)
    }

    /// What · call · where, in mono, with Copy bottom-right.
    private var detailsBlock: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(error.details.enumerated()), id: \.offset) { _, line in
                Text(line)
                    .font(theme.font(Self.detailText, design: .monospaced))
                    .lineSpacing(Self.detailLine - Self.detailText - 3)
                    .foregroundStyle(theme.muted)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack {
                Spacer(minLength: 0)
                Button {
                    UIPasteboard.general.string = error.copyText
                } label: {
                    Text("Copy")
                        .font(theme.font(Theme.tiny, weight: .semibold))
                        .foregroundStyle(theme.text)
                        .padding(.horizontal, 10)
                        .frame(height: Self.copyHeight)
                        .background(Capsule().fill(pillFill)) // control-size: exception inlineChips
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("\(id)-copy")
            }
            .padding(.top, 8)
        }
        .padding(.horizontal, 12)
        .padding(.top, 10)
        .padding(.bottom, 8)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(theme.codeChip))
        .accessibilityIdentifier("\(id)-details-block")
    }
}

/// The toast layer (Jacob 2026-10-26): the app's transient notices at the top of the screen,
/// under the status bar, 16 inset — an info notice as the plain card (6 s, tap dismisses) and a
/// failure as `ErrorNote` in its toast style (10 s, a touch holds it, × dismisses, Try again
/// re-runs the action). Replaces the root `.alert(item: $model.notice)`.
struct ToastHost: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme

    var body: some View {
        VStack(spacing: 8) {
            ForEach(model.toasts) { toast in
                ToastRow(toast: toast)
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 8)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .allowsHitTesting(!model.toasts.isEmpty)
        .animation(.easeOut(duration: 0.2), value: model.toasts)
    }
}

private struct ToastRow: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let toast: Toast

    var body: some View {
        if let error = toast.error {
            ErrorNote(
                id: "toast-\(toast.id.uuidString.prefix(8))",
                error: error,
                detailsOpen: Binding(get: { toast.detailsOpen }, set: { model.setToastDetails(toast.id, open: $0) }),
                style: .toast,
                onTryAgain: toast.hasRetry ? { model.retryToast(toast.id) } : nil,
                onSignIn: error.action == .signIn ? { model.dismissToast(toast.id); model.signOutForReauth() } : nil,
                onDismiss: { model.dismissToast(toast.id) }
            )
            .onLongPressGesture(minimumDuration: 0.05, pressing: { pressing in model.holdToast(toast.id, held: pressing) }, perform: {})
            .transition(.move(edge: .top).combined(with: .opacity))
        } else {
            Text(toast.text)
                .font(theme.font(Theme.small))
                .foregroundStyle(theme.text)
                .padding(.horizontal, 14)
                .padding(.vertical, 12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: ErrorNote.radius, style: .continuous).fill(theme.card))
                .overlay(RoundedRectangle(cornerRadius: ErrorNote.radius, style: .continuous).strokeBorder(theme.separator, lineWidth: 1))
                .shadow(color: Color.black.opacity(0.18), radius: 14, y: 6)
                .contentShape(Rectangle())
                .onTapGesture { model.dismissToast(toast.id) }
                .accessibilityIdentifier("toast-info")
                .transition(.move(edge: .top).combined(with: .opacity))
        }
    }
}

/// A sheet's note (Jacob 2026-10-26): the failure the sheet claimed from the toast stack while
/// it is up (`AppModel.claimedFailure`), else its own validation as a refusal — one element at
/// the top of the body. `claimFailures` on the sheet names the verbs it owns.
struct SheetErrorNote: View {
    @EnvironmentObject private var model: AppModel
    let id: String
    var local: UserError?
    var onTryAgain: (() -> Void)? = nil
    @State private var localOpen = false

    var body: some View {
        if let claimed = model.claimedFailure {
            ErrorNote(id: id, error: claimed, detailsOpen: $model.claimedFailureDetailsOpen, onTryAgain: onTryAgain)
        } else if let local {
            ErrorNote(id: id, error: local, detailsOpen: $localOpen)
        }
    }
}

extension View {
    /// While this sheet is up, failures under `verbs` (prefix match) draw in its `SheetErrorNote`
    /// instead of a toast.
    func claimFailures(_ surface: String, _ verbs: [String]) -> some View {
        modifier(ClaimFailures(surface: surface, verbs: verbs))
    }
}

private struct ClaimFailures: ViewModifier {
    @EnvironmentObject private var model: AppModel
    let surface: String
    let verbs: [String]

    func body(content: Content) -> some View {
        content
            .onAppear { model.failureClaim = FailureClaim(surface: surface, verbs: verbs) }
            .onDisappear { if model.failureClaim?.surface == surface { model.failureClaim = nil } }
    }
}
