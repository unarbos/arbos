// Wallet surfaces: the Settings page, the once-only phrase sheet, the restore sheet, the cheque
// card in a chat and the sheet that writes one. The model side is Sync/Wallet.swift.

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
                SettingsGroup(footer: "Anyone you chat with can send you TAO cheques. Off: your address stays private.") {
                    Toggle("Get paid by @\(model.user?.login ?? "handle")", isOn: Binding(get: { payable }, set: setPayable))
                        .tint(theme.accent)
                        .padding(.horizontal, 16)
                        .frame(minHeight: SettingsRow.height)
                }
                SettingsGroup(footer: "The phrase opens your encrypted chats on a new device and controls your TAO. It is in your iCloud Keychain; anyone who sees it can take both.") {
                    SettingsValueRow("Chat history backup", value: Self.backupLabel(backup))
                    SettingsActionRow(title: showPhrase ? "Hide recovery phrase" : "Show recovery phrase") { showPhrase.toggle() }
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
        VStack(spacing: 14) {
            LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], alignment: .leading, spacing: 10) {
                ForEach(Array(words.enumerated()), id: \.offset) { i, word in
                    HStack(spacing: 8) {
                        Text("\(i + 1)").font(theme.font(Theme.small)).foregroundStyle(theme.muted).frame(width: 20, alignment: .trailing)
                        Text(word).font(.system(.body, design: .monospaced)).foregroundStyle(theme.text)
                    }
                }
            }
            Button(action: copy) {
                Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc")
                    .font(theme.font(Theme.small, weight: .medium))
                    .contentTransition(.symbolEffect(.replace))
                    .foregroundStyle(copied ? theme.online : theme.muted)
                    .padding(.horizontal, 14)
                    .frame(height: 32)
                    .background(Capsule().fill(theme.bg.opacity(0.6)))
                    .overlay(Capsule().strokeBorder(theme.separator, lineWidth: 0.5))
            }
            .buttonStyle(.plain)
            .accessibilityLabel(copied ? "Copied" : "Copy recovery phrase")
            .accessibilityIdentifier("wallet-phrase-copy")
        }
        .padding(16)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(theme.card))
        .privacySensitive()
        .accessibilityIdentifier("wallet-phrase")
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

/// A cheque someone sent. Everything shown is read from the signed bytes; the message's text is
/// never trusted for the amount or the payee.
struct ChequeCard: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let bytes: String
    let mine: Bool
    @State private var cheque: WalletCheque?
    @State private var note: String?
    @State private var cashing = false
    @State private var cashed = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "signature").foregroundStyle(theme.accent)
                Text("TAO cheque").font(theme.font(Theme.small, weight: .medium)).foregroundStyle(theme.muted)
            }
            Text(cheque.map { walletFormatTao(rao: $0.rao) } ?? "—")
                .font(theme.font(28, weight: .semibold)).foregroundStyle(theme.text)
            if let note {
                Text(note).font(theme.font(Theme.small)).foregroundStyle(theme.muted)
            } else if cashed {
                Label("Cashed", systemImage: "checkmark.circle.fill").font(theme.font(Theme.small)).foregroundStyle(theme.online)
            } else if let c = cheque, !mine, c.to == model.walletAddressValue {
                Button(cashing ? "Cashing…" : "Cash it") { cash() }
                    .buttonStyle(.borderedProminent).tint(theme.accent).disabled(cashing)
            } else if mine {
                Text("Signed offline · cashes when either of you is online").font(theme.font(Theme.small)).foregroundStyle(theme.muted)
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

    private func read() async {
        cashed = model.isCashed(bytes)
        if model.chequeSnapshot == nil { await model.refreshChequeSnapshot() }
        guard let snapshot = model.chequeSnapshot else {
            note = "Go online once to read cheques."
            return
        }
        do {
            cheque = try walletReadCheque(snapshotJson: snapshot, bytes: bytes)
        } catch {
            note = "This cheque can't be read."
        }
    }

    private func cash() {
        cashing = true
        Task {
            if let why = await model.cashCheque(bytes) {
                note = "Bounced: \(why)"
            } else {
                cashed = true
            }
            cashing = false
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
    @State private var error: String?
    @State private var sending = false
    @FocusState private var focused: Bool

    private var valid: Bool { (try? walletParseTao(amount: amount)) != nil }

    var body: some View {
        VStack(spacing: 20) {
            SheetHeaderBar(title: "Send TAO", leading: .cancel, onLeading: { dismiss() },
                        action: sending ? "…" : "Sign", actionEnabled: valid && !sending, onAction: send)
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                TextField("0", text: $amount)
                    .keyboardType(.decimalPad)
                    .multilineTextAlignment(.trailing)
                    .font(theme.font(48, weight: .semibold))
                    .focused($focused)
                Text("τ").font(theme.font(32, weight: .medium)).foregroundStyle(theme.muted)
            }
            .padding(.horizontal, 32)
            Text("A cheque is signed on this phone, no network needed. They cash it whenever either of you is online.")
                .font(theme.font(Theme.small)).foregroundStyle(theme.muted).multilineTextAlignment(.center)
            if let error { Text(error).font(theme.font(Theme.small)).foregroundStyle(theme.danger) }
            Spacer(minLength: 0)
        }
        .padding(20)
        .background(theme.bg)
        .onAppear { focused = true }
    }

    private func send() {
        sending = true
        Task {
            error = await model.sendCheque(chatId: chatId, amount: amount)
            sending = false
            if error == nil { dismiss() }
        }
    }
}
