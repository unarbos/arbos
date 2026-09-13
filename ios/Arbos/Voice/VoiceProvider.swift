import Foundation

/// Build-time switches. Set `OPENAI_REALTIME` in
/// `SWIFT_ACTIVE_COMPILATION_CONDITIONS` to offer OpenAI in Settings.
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

    /// The providers Settings offers.
    static var available: [VoiceProvider] {
        allCases.filter { $0 != .openAIRealtime || FeatureFlags.openAIRealtime }
    }

    var label: String {
        switch self {
        case .selfHosted: return "Self-hosted"
        case .openAIRealtime: return "OpenAI Realtime"
        }
    }

    /// What the idle screen says while this provider cannot place a call.
    @MainActor
    func unconfiguredLabel(_ settings: AppSettings) -> String {
        switch self {
        case .selfHosted: return settings.selfHostedURL.isEmpty ? "no server set" : "no token set"
        case .openAIRealtime: return "no key set"
        }
    }

    @MainActor
    func isConfigured(_ settings: AppSettings) -> Bool {
        switch self {
        case .selfHosted: return !settings.selfHostedURL.isEmpty && !settings.voiceToken.isEmpty
        case .openAIRealtime: return !settings.openAIKey.isEmpty
        }
    }

    @MainActor
    func makeSession(_ settings: AppSettings) -> VoiceSession {
        switch self {
        case .selfHosted:
            return SelfHostedVoiceSession(serverURL: settings.selfHostedURL, token: settings.voiceToken)
        case .openAIRealtime:
            return OpenAIRealtimeSession(
                apiKey: settings.openAIKey,
                model: settings.openAIModel,
                instructions: AppSettings.callInstructions
            )
        }
    }
}
