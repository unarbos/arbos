import Foundation

/// What `scripts/gen-secrets.sh` baked into this build, if anything. A
/// dev build carries the live endpoints and tokens so the first launch
/// works without typing; a build without the file just starts empty.
struct BuiltInSecrets {
    var voiceServerURL = ""
    var voiceToken = ""
    var kernelURL = ""
    var kernelToken = ""
    var hubURL = ""
    var hubToken = ""

    static let current: BuiltInSecrets = {
        var secrets = BuiltInSecrets()
        guard let url = Bundle.main.url(forResource: "Secrets", withExtension: "plist"),
              let data = try? Data(contentsOf: url),
              let dict = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any] else {
            return secrets
        }
        func value(_ key: String) -> String {
            (dict[key] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        }
        secrets.voiceServerURL = value("voiceServerURL")
        secrets.voiceToken = value("voiceToken")
        secrets.kernelURL = value("kernelURL")
        secrets.kernelToken = value("kernelToken")
        secrets.hubURL = value("hubURL")
        secrets.hubToken = value("hubToken")
        return secrets
    }()
}
