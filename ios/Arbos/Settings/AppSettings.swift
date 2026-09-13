import Foundation

/// Everything the user can change. Plain values live in UserDefaults;
/// secrets live in the Keychain and are only mirrored here in memory.
@MainActor
final class AppSettings: ObservableObject {
    private static let openAIKeyAccount = "openai-api-key"
    private static let voiceTokenAccount = "voice-server-token"
    private static let kernelTokenAccount = "kernel-token"

    /// Interim endpoints: Cloudflare quick tunnels in front of the
    /// `voice-server/` gateway and the phone kernel. `voice.arbos.life` and
    /// `kernel-api.arbos.life` replace them once DNS is set. When a tunnel
    /// restarts, `EndpointDirectory` finds the new name.
    static let defaultVoiceServerURL = "wss://inline-voice-occupations-ultram.trycloudflare.com/ws"
    static let defaultKernelURL = "wss://live-got-person-permits.trycloudflare.com/"

    static let callInstructions = """
    You are Arbos, Jacob's agent. He is talking to you by voice while out \
    running. Answer in one or two short sentences. No lists, no preamble, \
    no sign-off. If you need to act on his Arbos project, say what you will \
    do in a few words and do it.
    """

    @Published var provider: VoiceProvider {
        didSet { defaults.set(provider.rawValue, forKey: "voiceProvider") }
    }
    @Published var openAIModel: String {
        didSet { defaults.set(openAIModel, forKey: "openAIModel") }
    }
    /// `wss://host/ws`. The token is added at connect time.
    @Published var selfHostedURL: String {
        didSet { defaults.set(selfHostedURL, forKey: "selfHostedURL") }
    }
    /// `wss://host/`. The token goes in the `Authorization` header.
    @Published var kernelURL: String {
        didSet { defaults.set(kernelURL, forKey: "kernelURL") }
    }
    @Published private(set) var openAIKey: String
    @Published private(set) var voiceToken: String
    @Published private(set) var kernelToken: String

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let stored = VoiceProvider(rawValue: defaults.string(forKey: "voiceProvider") ?? "") ?? .selfHosted
        provider = VoiceProvider.available.contains(stored) ? stored : .selfHosted
        openAIModel = defaults.string(forKey: "openAIModel") ?? "gpt-realtime"
        selfHostedURL = defaults.string(forKey: "selfHostedURL") ?? Self.defaultVoiceServerURL
        kernelURL = defaults.string(forKey: "kernelURL") ?? Self.defaultKernelURL
        openAIKey = Keychain.read(Self.openAIKeyAccount) ?? ""
        #if DEBUG
        // `-voiceToken …` / `-kernelToken …` on the launch line land in the
        // argument domain of UserDefaults (never on disk). Move them into
        // the Keychain so a scripted run can be configured.
        for (key, account) in [("voiceToken", Self.voiceTokenAccount), ("kernelToken", Self.kernelTokenAccount)] {
            if let injected = defaults.string(forKey: key), !injected.isEmpty {
                Keychain.write(injected, account: account)
            }
        }
        #endif
        voiceToken = Keychain.read(Self.voiceTokenAccount) ?? ""
        kernelToken = Keychain.read(Self.kernelTokenAccount) ?? ""
    }

    var isConfigured: Bool { provider.isConfigured(self) }

    /// Where the kernel listens, once both the URL and the token are set.
    var kernelEndpoint: ArbosKernelClient.Endpoint? {
        guard !kernelToken.isEmpty, let url = URL(string: kernelURL), let scheme = url.scheme,
              ["ws", "wss"].contains(scheme.lowercased()) else { return nil }
        return .init(url: url, token: kernelToken)
    }

    func saveOpenAIKey(_ key: String) {
        Keychain.write(key, account: Self.openAIKeyAccount)
        openAIKey = Keychain.read(Self.openAIKeyAccount) ?? ""
    }

    func saveVoiceToken(_ token: String) {
        Keychain.write(token, account: Self.voiceTokenAccount)
        voiceToken = Keychain.read(Self.voiceTokenAccount) ?? ""
    }

    func saveKernelToken(_ token: String) {
        Keychain.write(token, account: Self.kernelTokenAccount)
        kernelToken = Keychain.read(Self.kernelTokenAccount) ?? ""
    }
}
