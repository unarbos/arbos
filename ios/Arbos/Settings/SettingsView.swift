import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @Environment(\.dismiss) private var dismiss
    @State private var keyDraft = ""
    @State private var tokenDraft = ""
    @State private var kernelTokenDraft = ""
    @State private var hubTokenDraft = ""

    /// "0.2.0 (920)" — what TestFlight calls this build.
    private static var buildLine: String {
        let info = Bundle.main.infoDictionary ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? "?"
        let build = info["CFBundleVersion"] as? String ?? "?"
        return "\(version) (\(build))"
    }

    @EnvironmentObject private var notifier: Notifier

    var body: some View {
        NavigationStack {
            // Every section carries its own row colour. `scrollContentBackground`
            // hides the page behind the Form but not the fill behind each row,
            // and the same modifier on the Form does not reach them: sampled,
            // the sheet was 45% `#2c2c2e` — iOS's grouped-cell grey, in no
            // part of this app's palette — against a list screen that is 92%
            // `#161514`. One screen standing on a different ground.
            Form {
                Section {
                    if VoiceProvider.available.count > 1 {
                        Picker("Provider", selection: $settings.provider) {
                            ForEach(VoiceProvider.available) { provider in
                                Text(provider.label).tag(provider)
                            }
                        }
                    }
                    switch settings.provider {
                    case .selfHosted:
                        TextField("wss://host/ws", text: $settings.selfHostedURL)
                            .keyboardType(.URL)
                            .autocorrectionDisabled()
                            .textInputAutocapitalization(.never)
                        // `.oneTimeCode`, not `.password`: a token is not a
                        // site password, and `.password` had iOS put up its
                        // own "Save Password?" sheet over the list after every
                        // edit (M-122).
                        SecureField(tokenPlaceholder, text: $tokenDraft)
                            .textContentType(.oneTimeCode)
                            .autocorrectionDisabled()
                    case .openAIRealtime:
                        SecureField(keyPlaceholder, text: $keyDraft)
                            .textContentType(.oneTimeCode)
                            .autocorrectionDisabled()
                        TextField("Model", text: $settings.openAIModel)
                            .autocorrectionDisabled()
                            .textInputAutocapitalization(.never)
                    }
                } header: {
                    Text("Voice")
                } footer: {
                    Text(voiceFooter)
                }
                .listRowBackground(ArbosTheme.card)
                Section {
                    TextField("wss://host/", text: $settings.kernelURL)
                        .keyboardType(.URL)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                    SecureField(kernelTokenPlaceholder, text: $kernelTokenDraft)
                        .textContentType(.oneTimeCode)
                        .autocorrectionDisabled()
                } header: {
                    Text("Arbos kernel")
                } footer: {
                    Text("The main chat: history and streaming replies come from here. The voice server talks to the same kernel.")
                }
                .listRowBackground(ArbosTheme.card)
                Section {
                    TextField("wss://hub-host", text: $settings.hubURL)
                        .keyboardType(.URL)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                    SecureField(hubTokenPlaceholder, text: $hubTokenDraft)
                        .textContentType(.oneTimeCode)
                        .autocorrectionDisabled()
                } header: {
                    Text("Mesh hub")
                } footer: {
                    Text("Lists the machines and projects the Projects screen shows.")
                }
                .listRowBackground(ArbosTheme.card)
                Section {
                    Text(notifier.pushLine)
                        .font(ArbosTheme.caption)
                        .foregroundStyle(ArbosTheme.textMuted)
                } header: {
                    Text("Notifications")
                } footer: {
                    Text("Replies, questions and failures while the app is away. What you missed is shown when you come back either way.")
                }
                .listRowBackground(ArbosTheme.card)
                Section {
                    LabeledContent("Arbos", value: Self.buildLine)
                        .foregroundStyle(ArbosTheme.textMuted)
                } footer: {
                    Text("The TestFlight build on this phone.")
                }
                .listRowBackground(ArbosTheme.card)
            }
            .scrollContentBackground(.hidden)
            .background(ArbosTheme.bg)
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        if !keyDraft.isEmpty { settings.saveOpenAIKey(keyDraft) }
                        if !tokenDraft.isEmpty { settings.saveVoiceToken(tokenDraft) }
                        if !kernelTokenDraft.isEmpty { settings.saveKernelToken(kernelTokenDraft) }
                        if !hubTokenDraft.isEmpty { settings.saveHubToken(hubTokenDraft) }
                        dismiss()
                    }
                }
            }
        }
    }

    private var voiceFooter: String {
        switch settings.provider {
        case .selfHosted: return "Our speech server. Token stored in the Keychain; it rides in the URL as ?token=."
        case .openAIRealtime: return "Stored in the Keychain. Replies come from the model, not the kernel."
        }
    }

    private var keyPlaceholder: String {
        settings.openAIKey.isEmpty ? "OpenAI API key" : "API key saved · paste to replace"
    }

    private var tokenPlaceholder: String {
        settings.voiceToken.isEmpty ? "Server token" : "Token saved · paste to replace"
    }

    private var kernelTokenPlaceholder: String {
        settings.kernelToken.isEmpty ? "Kernel token" : "Token saved · paste to replace"
    }

    private var hubTokenPlaceholder: String {
        settings.hubToken.isEmpty ? "Hub client token" : "Token saved · paste to replace"
    }
}
