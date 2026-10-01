import Foundation

enum FeatureFlags {
    #if OPENAI_REALTIME
    static let openAIRealtime = true
    #else
    static let openAIRealtime = false
    #endif
}

enum VoiceProvider: String, CaseIterable, Identifiable {
    case selfHosted
    case openAIRealtime

    var id: String { rawValue }

    static var available: [VoiceProvider] {
        // Orb product: only the self-hosted GPT-Live + Codex path.
        [.selfHosted]
    }

    var label: String {
        switch self {
        case .selfHosted: return "Server"
        case .openAIRealtime: return "OpenAI Realtime"
        }
    }

    @MainActor
    func unconfiguredLabel(_ settings: AppSettings) -> String {
        settings.unconfiguredLabel
    }

    @MainActor
    func isConfigured(_ settings: AppSettings) -> Bool {
        settings.isConfigured
    }

    @MainActor
    func makeSession(_ settings: AppSettings) -> VoiceSession {
        switch self {
        case .selfHosted:
            return SelfHostedVoiceSession(
                serverURL: settings.selfHostedURL,
                token: settings.voiceToken,
                openAIKey: settings.openAIKey,
                project: nil
            )
        case .openAIRealtime:
            return OpenAIRealtimeSession(
                apiKey: settings.openAIKey,
                model: settings.openAIModel,
                instructions: AppSettings.callInstructions
            )
        }
    }
}
