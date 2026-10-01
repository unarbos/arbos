import Foundation

/// Everything the user can change. Plain values live in UserDefaults;
/// the OpenAI key lives in the Keychain and is only mirrored here in memory.
///
/// Orb product: server URL + OpenAI API key. Nothing else.
@MainActor
final class AppSettings: ObservableObject {
    private static let openAIKeyAccount = "openai-api-key"
    private static let voiceTokenAccount = "voice-server-token"
    private static let kernelTokenAccount = "kernel-token"
    private static let hubTokenAccount = "hub-token"

    static let defaultVoiceServerURL = ""

    static let callInstructions = """
    You are a brief voice companion. Answer greetings in one short sentence. \
    Anything about code, files, projects, or work: say you will check and wait \
    for the result. Never invent results.
    """

    /// Always self-hosted voice server (GPT-Live on the machine). Kept so
    /// older CallViewModel / VoiceLink paths still compile.
    @Published var provider: VoiceProvider {
        didSet { defaults.set(provider.rawValue, forKey: "voiceProvider") }
    }
    @Published var openAIModel: String {
        didSet { defaults.set(openAIModel, forKey: "openAIModel") }
    }
    /// `wss://host/ws` — the voice server that runs GPT-Live + Codex.
    @Published var selfHostedURL: String {
        didSet { defaults.set(selfHostedURL, forKey: "selfHostedURL") }
    }
    @Published var kernelURL: String {
        didSet { defaults.set(kernelURL, forKey: "kernelURL") }
    }
    @Published var hubURL: String {
        didSet { defaults.set(hubURL, forKey: "hubURL") }
    }
    @Published var frontProject: String? {
        didSet { defaults.set(frontProject, forKey: "frontProject") }
    }
    @Published var kernelTarget: KernelTarget {
        didSet { defaults.set(kernelTarget.stored, forKey: "kernelTarget") }
    }
    @Published private(set) var openAIKey: String
    @Published private(set) var voiceToken: String
    @Published private(set) var kernelToken: String
    @Published private(set) var hubToken: String

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        provider = .selfHosted
        openAIModel = defaults.string(forKey: "openAIModel") ?? "gpt-live-1"
        let baked = BuiltInSecrets.current
        let storedURL = defaults.string(forKey: "selfHostedURL") ?? ""
        if !storedURL.isEmpty {
            selfHostedURL = storedURL
        } else if !baked.voiceServerURL.isEmpty {
            selfHostedURL = baked.voiceServerURL
        } else {
            selfHostedURL = Self.defaultVoiceServerURL
        }
        // When the bake moves the server URL and the user had not typed over
        // the previous bake, follow it (quick tunnels rotate).
        if !baked.voiceServerURL.isEmpty, !ProcessInfo.processInfo.arguments.contains("-selfHostedURL") {
            let previous = defaults.string(forKey: "baked-voiceServerURL")
            if previous == nil || selfHostedURL == previous || selfHostedURL.isEmpty {
                selfHostedURL = baked.voiceServerURL
                defaults.set(baked.voiceServerURL, forKey: "selfHostedURL")
            }
            defaults.set(baked.voiceServerURL, forKey: "baked-voiceServerURL")
        }
        kernelURL = defaults.string(forKey: "kernelURL") ?? baked.kernelURL
        hubURL = defaults.string(forKey: "hubURL") ?? baked.hubURL
        kernelTarget = KernelTarget(stored: defaults.string(forKey: "kernelTarget") ?? "pod")
        frontProject = defaults.string(forKey: "frontProject")
        openAIKey = Keychain.read(Self.openAIKeyAccount) ?? ""
        voiceToken = Keychain.read(Self.voiceTokenAccount) ?? baked.voiceToken
        kernelToken = Keychain.read(Self.kernelTokenAccount) ?? baked.kernelToken
        hubToken = Keychain.read(Self.hubTokenAccount) ?? baked.hubToken
    }

    var hubConfigured: Bool {
        !hubToken.isEmpty && URL(string: hubURL)?.host != nil
    }

    var chatEndpoint: ArbosKernelClient.Endpoint? {
        switch kernelTarget {
        case .pod:
            return kernelEndpoint
        case .hub(let machine, let project):
            guard hubConfigured, let url = HubClient.attachURL(hubURL: hubURL, machine: machine, project: project) else {
                return nil
            }
            return .init(url: url, token: hubToken)
        }
    }

    /// Server URL and OpenAI key — the only things Settings asks for.
    var isConfigured: Bool {
        !selfHostedURL.isEmpty && !openAIKey.isEmpty
    }

    var unconfiguredLabel: String {
        if selfHostedURL.isEmpty { return "set the server in Settings" }
        if openAIKey.isEmpty { return "set your OpenAI key in Settings" }
        return ""
    }

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

    func saveHubToken(_ token: String) {
        Keychain.write(token, account: Self.hubTokenAccount)
        hubToken = Keychain.read(Self.hubTokenAccount) ?? ""
    }
}
