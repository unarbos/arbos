// Wallet surfaces: the Settings page, the once-only phrase sheet, the restore sheet, the cheque
// card in a chat and the sheet that writes one. The model side is Sync/Wallet.swift.

import LocalAuthentication
import SwiftUI
import UIKit
import UniformTypeIdentifiers
import ArbosFFI

// MARK: Settings → Wallet

struct WalletPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @State private var balance: String?
    @State private var backup: String?
    @State private var showPhrase = false
    @State private var payable = UserDefaults.standard.bool(forKey: "walletPayable")
    @State private var error: String?

    var body: some View {
        SettingsPage(title: "Wallet") {
            if let address = model.walletAddressValue {
                SettingsGroup(footer: "Your TAO account on Bittensor. It works in any Bittensor wallet with the same recovery phrase.") {
                    SettingsValueRow("Balance", value: balance ?? "—")
                    SettingsValueRow("Address") {
                        Button {
                            UIPasteboard.general.string = address
                        } label: {
                            Text(Self.short(address)).font(.system(.body, design: .monospaced)).foregroundStyle(theme.accent)
                        }
                        .accessibilityLabel("Copy address")
                    }
                }
                // The link exists only so people can send cheques to a handle.
                if Wallet.chequesEnabled {
                    SettingsGroup(footer: "People you have a direct chat with can see your address and send you TAO cheques. Off: your address stays private.") {
                        Toggle("Get paid by @\(model.user?.login ?? "handle")", isOn: Binding(get: { payable }, set: setPayable))
                            .tint(theme.accent)
                            .padding(.horizontal, 16)
                            .frame(minHeight: SettingsRow.height)
                    }
                }
                SettingsGroup(footer: "The phrase opens your encrypted chats on a new device and controls your TAO. It is in your iCloud Keychain; anyone who sees it can take both.") {
                    SettingsValueRow("Chat history backup", value: Self.backupLabel(backup))
                    SettingsActionRow(title: showPhrase ? "Hide recovery phrase" : "Show recovery phrase") { showPhrase ? (showPhrase = false) : revealPhrase() }
                }
                if showPhrase, let phrase = model.walletPhrase {
                    PhraseGrid(phrase: phrase).padding(.horizontal, 16)
                }
            } else {
                SettingsGroup(footer: "This device doesn't have your recovery phrase yet.") {
                    SettingsActionRow(title: "Enter recovery phrase") { model.walletPrompt = .restore }
                }
            }
            if let error {
                Text(error).font(theme.font(Theme.small)).foregroundStyle(theme.danger).padding(.horizontal, 16)
            }
        }
        .task {
            if let address = model.walletAddressValue {
                balance = (try? await walletBalance(address: address)).map { walletFormatTao(rao: $0) }
                await model.refreshChequeSnapshot()
            }
            backup = await model.matrixClient?.backup()
        }
    }

    /// The phrase (and its Copy button) only appears to the phone's owner: Face ID, Touch ID or
    /// the passcode. A phone with no passcode cannot show it.
    private func revealPhrase() {
        error = nil
        let context = LAContext()
        guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: nil) else {
            error = "Set a passcode on this iPhone to see your recovery phrase."
            return
        }
        Task {
            showPhrase = (try? await context.evaluatePolicy(.deviceOwnerAuthentication,
                                                            localizedReason: "Show your recovery phrase")) ?? false
        }
    }

    private func setPayable(_ on: Bool) {
        guard let phrase = model.walletPhrase, let me = model.user,
              let address = try? walletAddress(phrase: phrase) else { return }
        payable = on
        error = nil
        Task {
            do {
                if on {
                    try await model.relay.linkWallet(address: address, proof: walletLinkProof(phrase: phrase, userId: me.id))
                } else {
                    try await model.relay.unlinkWallet()
                }
                UserDefaults.standard.set(on, forKey: "walletPayable")
            } catch {
                payable = !on
                self.error = "Couldn't save that. \(error.localizedDescription)"
            }
        }
    }

    static func short(_ address: String) -> String {
        address.count > 12 ? "\(address.prefix(6))…\(address.suffix(6))" : address
    }

    static func backupLabel(_ state: String?) -> String {
        switch state {
        case "on": return "On"
        case "locked": return "Needs your phrase"
        case "off": return "Off"
        default: return "—"
        }
    }
}

/// Twelve numbered words, two columns, easy to copy onto paper, and one Copy button underneath.
struct PhraseGrid: View {
    @Environment(\.theme) private var theme
    let phrase: String
    @State private var copied = false

    /// The phrase stays on this phone (no Universal Clipboard to a nearby Mac) and leaves the
    /// pasteboard after a minute, long enough to paste into a password manager.
    static func copyToPasteboard(_ phrase: String) {
        UIPasteboard.general.setItems([[UTType.utf8PlainText.identifier: phrase]],
                                      options: [.localOnly: true, .expirationDate: Date.now.addingTimeInterval(60)])
    }

    var body: some View {
        let words = phrase.split(separator: " ").map(String.init)
        VStack(spacing: 20) {
            // Sized to the words and centred, so the block and the Copy pill share one axis.
            Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 12) {
                ForEach(0..<(words.count + 1) / 2, id: \.self) { row in
                    GridRow {
                        word(row * 2, words, gap: 28)
                        word(row * 2 + 1, words, gap: 0)
                    }
                }
            }
            .frame(maxWidth: .infinity)
            Button(action: copy) {
                HStack(spacing: 6) {
                    Image(systemName: copied ? "checkmark" : "square.on.square")
                        .font(.system(size: 13, weight: .medium))
                        .contentTransition(.symbolEffect(.replace))
                    Text(copied ? "Copied" : "Copy")
                }
                .font(theme.font(Theme.small, weight: .medium))
                .foregroundStyle(copied ? theme.online : theme.muted)
                .frame(width: 112, height: 34)
                .background(Capsule().fill(theme.bg.opacity(0.6)))
                .overlay(Capsule().strokeBorder(theme.separator, lineWidth: 0.5))
                .contentShape(Capsule())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(copied ? "Copied" : "Copy recovery phrase")
            .accessibilityIdentifier("wallet-phrase-copy")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 22)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(theme.card))
        .privacySensitive()
        .accessibilityIdentifier("wallet-phrase")
    }

    /// One numbered word as two grid cells: the number right-aligned in its own column.
    @ViewBuilder private func word(_ i: Int, _ words: [String], gap: CGFloat) -> some View {
        if i < words.count {
            Text("\(i + 1)").font(theme.font(Theme.small).monospacedDigit()).foregroundStyle(theme.muted)
                .gridColumnAlignment(.trailing)
            Text(words[i]).font(.system(.body, design: .monospaced)).foregroundStyle(theme.text)
                .padding(.trailing, gap)
        }
    }

    private func copy() {
        Self.copyToPasteboard(phrase)
        Haptics.pick()
        withAnimation(.snappy) { copied = true }
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(1.5))
            withAnimation(.snappy) { copied = false }
        }
    }
}

// MARK: The once-only sheets

struct WalletPromptSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let prompt: WalletPrompt
    @State private var typed = ""
    @State private var error: String?
    @State private var working = false

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            switch prompt {
            case .backup(let phrase):
                SheetHeaderBar(title: "Your recovery phrase", leading: .none, action: "Done", onAction: { model.walletPrompt = nil })
                Text("These 12 words unlock your encrypted chats on any new device, and they're your TAO wallet. They're saved in your iCloud Keychain — write them down too, and never share them.")
                    .font(theme.font(Theme.body)).foregroundStyle(theme.muted)
                PhraseGrid(phrase: phrase)
            case .restore:
                SheetHeaderBar(title: "Unlock your chats", leading: .cancel, onLeading: { model.walletPrompt = nil },
                            action: working ? "…" : "Unlock", actionEnabled: !working && !typed.isEmpty, onAction: unlock)
                Text("Your encrypted history is locked on this device. Enter the 12-word recovery phrase from your other device (Settings → Wallet).")
                    .font(theme.font(Theme.body)).foregroundStyle(theme.muted)
                TextField("twelve words…", text: $typed, axis: .vertical)
                    .lineLimit(3...4)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .font(.system(.body, design: .monospaced))
                    .padding(12)
                    .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(theme.field))
                if let error { Text(error).font(theme.font(Theme.small)).foregroundStyle(theme.danger) }
            }
            Spacer(minLength: 0)
        }
        .padding(20)
        .background(theme.bg)
        .interactiveDismissDisabled(prompt != .restore)
    }

    private func unlock() {
        working = true
        Task {
            error = await model.restoreWallet(typed)
            working = false
        }
    }
}

// MARK: Cheques in a chat

/// A cheque someone sent. Everything shown is read from the signed bytes, whose signature is
/// checked; the message's text is never trusted for the amount or the payee. A cheque is a
/// promise: the card says "not paid" until the chain has cashed it.
struct ChequeCard: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let bytes: String
    let mine: Bool
    @State private var cheque: WalletCheque?
    @State private var note: String?
    @State private var cashing = false
    @State private var cashed = false
    @State private var pending = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "signature").foregroundStyle(theme.accent)
                Text("TAO cheque").font(theme.font(Theme.small, weight: .medium)).foregroundStyle(theme.muted)
            }
            Text(cheque.map { walletFormatTao(rao: $0.rao) } ?? "—")
                .font(theme.font(28, weight: .semibold)).foregroundStyle(theme.text)
            if cashed {
                Label("Cashed", systemImage: "checkmark.circle.fill").font(theme.font(Theme.small)).foregroundStyle(theme.online)
            } else if pending {
                Label("Submitted · waiting for the chain", systemImage: "clock").font(theme.font(Theme.small)).foregroundStyle(theme.muted)
            } else if let note {
                Text(note).font(theme.font(Theme.small)).foregroundStyle(theme.muted)
            } else if let c = cheque, mine, model.chequeNumberSpent(c) {
                // The payer's side cannot tell which without searching the chain.
                Text("Cashed, or its number was used for another payment").font(theme.font(Theme.small)).foregroundStyle(theme.muted)
            } else if let c = cheque {
                let left = Self.timeLeft(expiresAt: c.expiresAt, now: Date())
                if Wallet.chequesEnabled, left != nil, !mine, c.to == model.walletAddressValue {
                    Button(cashing ? "Cashing…" : "Cash it") { cash() }
                        .buttonStyle(.borderedProminent).tint(theme.accent).disabled(cashing)
                }
                Text(left.map { "Not paid until \(mine ? "they cash it" : "cashed") · \($0)" } ?? "Expired · not paid")
                    .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
            }
        }
        .padding(16)
        .frame(maxWidth: 280, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.card))
        .overlay(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).strokeBorder(theme.separator, lineWidth: 1))
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("cheque-card")
        .task { await read() }
    }

    /// "expires in ~5 h" / "expires in ~20 min", or nil once it has expired.
    static func timeLeft(expiresAt: UInt64, now: Date) -> String? {
        let seconds = Double(expiresAt) - now.timeIntervalSince1970
        guard seconds > 0 else { return nil }
        return seconds >= 3600 ? "expires in ~\(Int(seconds / 3600)) h" : "expires in ~\(max(1, Int(seconds / 60))) min"
    }

    private func read() async {
        cashed = model.isCashed(bytes)
        let decode = { model.chequeSnapshot.flatMap { try? walletReadCheque(snapshotJson: $0, bytes: bytes) } }
        cheque = decode()
        // No snapshot yet, or the cheque was signed under a runtime newer than the saved one
        // knows (its signature then doesn't check): take a new one and look again.
        if cheque == nil, await model.refreshChequeSnapshot() { cheque = decode() }
        guard cheque != nil else {
            note = model.chequeSnapshot == nil ? "Go online once to read cheques." : "This cheque can't be read."
            return
        }
        // Left pending by an earlier attempt (a timeout, the app closed): look again.
        if !cashed, model.isChequePending(bytes) {
            pending = true
            await settle()
        }
    }

    private func cash() {
        cashing = true
        Task {
            await settle()
            cashing = false
        }
    }

    private func settle() async {
        switch await model.cashCheque(bytes) {
        case .paid:
            cashed = true
            pending = false
        case .pending:
            pending = true
        case .refused(let why):
            note = "Can't cash: \(why)"
            pending = false
        }
    }
}

/// "+" → Send TAO in a direct chat.
struct SendChequeSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let chatId: Id
    @State private var amount = ""
    @State private var payee: ChequePayee?
    @State private var error: String?
    @State private var sending = false
    @FocusState private var focused: Bool

    private var valid: Bool { (try? walletParseTao(amount: amount)) != nil }
    /// Nothing is signed until the address is on screen, and not while it is in question.
    private var signable: Bool { valid && !sending && payee != nil && payee?.changedTo == nil }

    var body: some View {
        VStack(spacing: 20) {
            SheetHeaderBar(title: "Send TAO", leading: .cancel, onLeading: { dismiss() },
                        action: sending ? "…" : "Sign", actionEnabled: signable, onAction: send)
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                TextField("0", text: $amount)
                    .keyboardType(.decimalPad)
                    .multilineTextAlignment(.trailing)
                    .font(theme.font(48, weight: .semibold))
                    .focused($focused)
                Text("τ").font(theme.font(32, weight: .medium)).foregroundStyle(theme.muted)
            }
            .padding(.horizontal, 32)
            if let payee {
                VStack(alignment: .leading, spacing: 6) {
                    Text("To \(payee.name)").font(theme.font(Theme.small, weight: .medium)).foregroundStyle(theme.muted)
                    // The whole address, never shortened: this is what gets signed.
                    Text(payee.address).font(.system(.footnote, design: .monospaced)).foregroundStyle(theme.text)
                        .textSelection(.enabled)
                        .accessibilityIdentifier("cheque-payee-address")
                    if let changed = payee.changedTo {
                        Label("\(payee.name)'s address changed", systemImage: "exclamationmark.triangle.fill")
                            .font(theme.font(Theme.small, weight: .medium)).foregroundStyle(theme.text)
                        Text("Above is the address this phone has always used for them. They now list a different one:")
                            .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                        Text(changed).font(.system(.footnote, design: .monospaced)).foregroundStyle(theme.text)
                            .textSelection(.enabled)
                        Text("Ask them, outside this app, whether they changed wallets before you pay the new address.")
                            .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                        Button("Address changed — use new address") { self.payee = model.acceptChangedPayee(payee) }
                            .buttonStyle(.bordered).tint(theme.accent)
                            .accessibilityIdentifier("cheque-payee-accept-change")
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(14)
                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(theme.card))
            }
            Text("A cheque is signed on this phone and pays nothing until it is cashed. They have about 7 hours to cash it, while either of you is online.")
                .font(theme.font(Theme.small)).foregroundStyle(theme.muted).multilineTextAlignment(.center)
            if let error { Text(error).font(theme.font(Theme.small)).foregroundStyle(theme.danger) }
            Spacer(minLength: 0)
        }
        .padding(20)
        .background(theme.bg)
        .onAppear { focused = true }
        .task {
            do { payee = try await model.chequePayee(chatId: chatId) } catch { self.error = walletWords(error) }
        }
    }

    private func send() {
        guard let to = payee?.address else { return }
        sending = true
        Task {
            error = await model.sendCheque(chatId: chatId, amount: amount, to: to)
            sending = false
            if error == nil { dismiss() }
        }
    }
}
