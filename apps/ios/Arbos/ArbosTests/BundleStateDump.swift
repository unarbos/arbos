import Foundation
import XCTest

/// The unit bundle runs inside the app (`TEST_HOST`), on the same simulator as the UI bundle (both
/// are `parallelizable = NO`), so whatever it leaves in the app's defaults domain or data container
/// is there when the UI cases launch the app. When the bundle finishes, this prints what it left to
/// the test log, one `bundle-state:` line each, between a `begin` and an `end` line. No `begin` line
/// means this observer did not load, not that nothing was left.
///
/// Values are printed only for string lists (row ids); any other value as its type and size.
/// Loaded as the bundle's principal class (`INFOPLIST_KEY_NSPrincipalClass`).
@objc(ArbosTestsPrincipal)
final class BundleStateDump: NSObject, XCTestObservation {
    override init() {
        super.init()
        XCTestObservationCenter.shared.addTestObserver(self)
    }

    func testBundleDidFinish(_ testBundle: Bundle) {
        var out = ["bundle-state: begin \(testBundle.bundleURL.lastPathComponent)"]
        let id = Bundle.main.bundleIdentifier ?? "?"
        let domain = UserDefaults.standard.persistentDomain(forName: id) ?? [:]
        out.append("bundle-state: defaults \(id) keys=\(domain.count)")
        for key in domain.keys.sorted() {
            out.append("bundle-state: default \(key) \(Self.describe(domain[key]!))")
        }
        let home = URL(fileURLWithPath: NSHomeDirectory()).standardizedFileURL
        let keys: [URLResourceKey] = [.fileSizeKey, .isRegularFileKey]
        var count = 0
        if let files = FileManager.default.enumerator(at: home, includingPropertiesForKeys: keys) {
            for case let url as URL in files {
                guard let values = try? url.resourceValues(forKeys: Set(keys)), values.isRegularFile == true else { continue }
                count += 1
                if count <= 400 {
                    let path = url.standardizedFileURL.path.replacingOccurrences(of: home.path + "/", with: "")
                    out.append("bundle-state: file \(path) \(values.fileSize ?? 0)")
                }
            }
        }
        out.append("bundle-state: files=\(count)\(count > 400 ? " (first 400 listed)" : "")")
        out.append("bundle-state: end")
        FileHandle.standardError.write(Data((out.joined(separator: "\n") + "\n").utf8))
    }

    private static func describe(_ value: Any) -> String {
        if let list = value as? [String] { return "[String] \(list.count) \(Array(list.prefix(10)))" }
        let size = (try? PropertyListSerialization.data(fromPropertyList: value, format: .binary, options: 0).count) ?? -1
        return "\(type(of: value)) \(size) bytes"
    }
}
