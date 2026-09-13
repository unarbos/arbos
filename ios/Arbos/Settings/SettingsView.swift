import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @Environment(\.dismiss) private var dismiss
    @State private var keyDraft = ""
    @State private var tokenDraft = ""
    @State private var kernelTokenDraft = ""

    var body: some View {
        NavigationStack {
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
                        SecureField(tokenPlaceholder, text: $tokenDraft)
                            .textContentType(.password)
                            .autocorrectionDisabled()
                    case .openAIRealtime:
                        SecureField(keyPlaceholder, text: $keyDraft)
                            .textContentType(.password)
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
                Section {
                    TextField("wss://host/", text: $settings.kernelURL)
                        .keyboardType(.URL)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                    SecureField(kernelTokenPlaceholder, text: $kernelTokenDraft)
                        .textContentType(.password)
                        .autocorrectionDisabled()
                } header: {
                    Text("Arbos kernel")
                } footer: {
                    Text("The main chat: history and streaming replies come from here. The voice server talks to the same kernel.")
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        if !keyDraft.isEmpty { settings.saveOpenAIKey(keyDraft) }
                        if !tokenDraft.isEmpty { settings.saveVoiceToken(tokenDraft) }
                        if !kernelTokenDraft.isEmpty { settings.saveKernelToken(kernelTokenDraft) }
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
}
