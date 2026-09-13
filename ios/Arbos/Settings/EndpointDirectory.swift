import Foundation

/// The live hosts, as published by the deploy scripts. Both the speech
/// server and the kernel sit behind Cloudflare quick tunnels whose names
/// change when the tunnel restarts; this file is where the new names land.
///
/// Format: the first non-comment line is the speech server's https origin;
/// a `kernel: wss://host/?token=…` line names the kernel. Tokens in the
/// file are placeholders and are dropped.
struct EndpointDirectory {
    static let url = URL(string: "https://raw.githubusercontent.com/unarbos/arbos/qa-results/voice-endpoint.txt")!

    var voiceServerURL: String?
    var kernelURL: String?
    var hubURL: String?

    static func fetch() async -> EndpointDirectory? {
        var request = URLRequest(url: url)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 8
        guard let (data, response) = try? await URLSession.shared.data(for: request),
              (response as? HTTPURLResponse)?.statusCode == 200,
              let text = String(data: data, encoding: .utf8) else { return nil }
        return parse(text)
    }

    static func parse(_ text: String) -> EndpointDirectory {
        var directory = EndpointDirectory()
        for raw in text.split(whereSeparator: \.isNewline) {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.isEmpty || line.hasPrefix("#") { continue }
            if line.hasPrefix("kernel:") {
                directory.kernelURL = stripToken(line.dropFirst("kernel:".count).trimmingCharacters(in: .whitespaces))
            } else if line.hasPrefix("hub:") {
                directory.hubURL = stripToken(line.dropFirst("hub:".count).trimmingCharacters(in: .whitespaces))
            } else if directory.voiceServerURL == nil, line.hasPrefix("http") {
                directory.voiceServerURL = line
                    .replacingOccurrences(of: "https://", with: "wss://")
                    .replacingOccurrences(of: "http://", with: "ws://")
                    .trimmingCharacters(in: CharacterSet(charactersIn: "/")) + "/ws"
            }
        }
        return directory
    }

    private static func stripToken(_ value: String) -> String? {
        guard var components = URLComponents(string: value.split(separator: " ").first.map(String.init) ?? value) else {
            return nil
        }
        components.queryItems = nil
        return components.url?.absoluteString
    }
}
