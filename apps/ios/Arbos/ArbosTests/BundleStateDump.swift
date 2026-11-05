import AVFoundation
import Foundation
import Photos
import Security
import UIKit
import UserNotifications
import XCTest

/// The unit bundle runs inside the app (`TEST_HOST`), on the same simulator as the UI bundle (both
/// are `parallelizable = NO`), so whatever it leaves in the app's defaults domain or data container
/// is there when the UI cases launch the app. When the bundle finishes, this prints what it left to
/// the test log, one `bundle-state:` line each, between a `begin` and an `end` line. No `begin` line
/// means this observer did not load, not that nothing was left.
///
/// Values are printed only for string lists (row ids); any other value as its type and size.
/// The keychain, the pasteboard and the app's privacy permissions outlive a launch too: keychain
/// items as class, service and count (never their data, and no account names: they are relay
/// URLs), the pasteboard as item count, types and change count (never its contents, so no paste
/// prompt), permissions as each status. Not covered: the rest of the simulator's privacy database
/// and other simulator-wide state; both are only readable from the host, after both bundles ran.
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
        out += Self.keychain()
        let board = UIPasteboard.general
        out.append("bundle-state: pasteboard items=\(board.numberOfItems) changeCount=\(board.changeCount) types=\(Array(board.types.prefix(10)))")
        out.append("bundle-state: privacy \(Self.privacy())")
        out.append("bundle-state: end")
        FileHandle.standardError.write(Data((out.joined(separator: "\n") + "\n").utf8))
    }

    private static func describe(_ value: Any) -> String {
        if let list = value as? [String] { return "[String] \(list.count) \(Array(list.prefix(10)))" }
        let size = (try? PropertyListSerialization.data(fromPropertyList: value, format: .binary, options: 0).count) ?? -1
        return "\(type(of: value)) \(size) bytes"
    }

    /// Attributes only: `kSecReturnData` is never set, so no secret is ever read, let alone printed.
    private static func keychain() -> [String] {
        let classes: [(String, CFString)] = [
            ("genp", kSecClassGenericPassword), ("inet", kSecClassInternetPassword),
            ("cert", kSecClassCertificate), ("keys", kSecClassKey), ("idnt", kSecClassIdentity),
        ]
        var lines: [String] = []
        for (name, secClass) in classes {
            let query: [String: Any] = [
                kSecClass as String: secClass,
                kSecMatchLimit as String: kSecMatchLimitAll,
                kSecReturnAttributes as String: true,
            ]
            var result: CFTypeRef?
            let status = SecItemCopyMatching(query as CFDictionary, &result)
            if status == errSecItemNotFound {
                lines.append("bundle-state: keychain \(name) count=0")
                continue
            }
            guard status == errSecSuccess, let items = result as? [[String: Any]] else {
                lines.append("bundle-state: keychain \(name) status=\(status)")
                continue
            }
            guard name == "genp" else {
                lines.append("bundle-state: keychain \(name) count=\(items.count)")
                continue
            }
            var byService: [String: Int] = [:]
            for item in items { byService[item[kSecAttrService as String] as? String ?? "-", default: 0] += 1 }
            for (service, n) in byService.sorted(by: { $0.key < $1.key }) {
                lines.append("bundle-state: keychain \(name) service=\(service) count=\(n)")
            }
        }
        return lines
    }

    /// Statuses only; none of these calls asks the user. The notification settings arrive on a
    /// background queue, so waiting on the main thread here cannot deadlock.
    private static func privacy() -> String {
        let camera = cameraStatus(AVCaptureDevice.authorizationStatus(for: .video))
        let microphone: String
        switch AVAudioApplication.shared.recordPermission {
        case .undetermined: microphone = "notDetermined"
        case .denied: microphone = "denied"
        case .granted: microphone = "granted"
        @unknown default: microphone = "unknown"
        }
        let photosAdd: String
        switch PHPhotoLibrary.authorizationStatus(for: .addOnly) {
        case .notDetermined: photosAdd = "notDetermined"
        case .restricted: photosAdd = "restricted"
        case .denied: photosAdd = "denied"
        case .authorized: photosAdd = "authorized"
        case .limited: photosAdd = "limited"
        @unknown default: photosAdd = "unknown"
        }
        var notifications = "unknown"
        let done = DispatchSemaphore(value: 0)
        UNUserNotificationCenter.current().getNotificationSettings { settings in
            switch settings.authorizationStatus {
            case .notDetermined: notifications = "notDetermined"
            case .denied: notifications = "denied"
            case .authorized: notifications = "authorized"
            case .provisional: notifications = "provisional"
            case .ephemeral: notifications = "ephemeral"
            @unknown default: notifications = "unknown"
            }
            done.signal()
        }
        // On a timeout the handler may still write `notifications` later, so it is not read.
        let answered = done.wait(timeout: .now() + 10) == .success
        return "camera=\(camera) microphone=\(microphone) photosAdd=\(photosAdd) notifications=\(answered ? notifications : "timeout")"
    }

    private static func cameraStatus(_ status: AVAuthorizationStatus) -> String {
        switch status {
        case .notDetermined: return "notDetermined"
        case .restricted: return "restricted"
        case .denied: return "denied"
        case .authorized: return "authorized"
        @unknown default: return "unknown"
        }
    }
}
