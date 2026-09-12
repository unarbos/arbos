import Foundation

/// Everything the user can change. Plain values live in UserDefaults; the
/// API key lives in the Keychain and is only mirrored here in memory.
@MainActor
final class AppSettings: ObservableObject {
    private static let openAIKeyAccount = "openai-api-key"

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
    @Published var selfHostedURL: String {
        didSet { defaults.set(selfHostedURL, forKey: "selfHostedURL") }
    }
    @Published var kernelHost: String {
        didSet { defaults.set(kernelHost, forKey: "kernelHost") }
    }
    @Published var kernelPort: Int {
        didSet { defaults.set(kernelPort, forKey: "kernelPort") }
    }
    @Published private(set) var openAIKey: String

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let stored = VoiceProvider(rawValue: defaults.string(forKey: "voiceProvider") ?? "") ?? .selfHosted
        provider = VoiceProvider.available.contains(stored) ? stored : .selfHosted
        openAIModel = defaults.string(forKey: "openAIModel") ?? "gpt-realtime"
        selfHostedURL = defaults.string(forKey: "selfHostedURL") ?? ""
        kernelHost = defaults.string(forKey: "kernelHost") ?? ""
        kernelPort = defaults.integer(forKey: "kernelPort")
        openAIKey = Keychain.read(Self.openAIKeyAccount) ?? ""
    }

    var isConfigured: Bool { provider.isConfigured(self) }

    /// Where the kernel listens, if the user filled it in.
    var kernelEndpoint: ArbosKernelClient.Endpoint? {
        guard !kernelHost.isEmpty, (1...65535).contains(kernelPort) else { return nil }
        return .init(host: kernelHost, port: UInt16(kernelPort))
    }

    func saveOpenAIKey(_ key: String) {
        Keychain.write(key, account: Self.openAIKeyAccount)
        openAIKey = Keychain.read(Self.openAIKeyAccount) ?? ""
    }
}
