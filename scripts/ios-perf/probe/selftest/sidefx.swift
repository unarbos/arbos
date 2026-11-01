import Foundation
import Combine
import Security
typealias ProbeFn = @convention(c) (Int32) -> Void
let probeFn: ProbeFn? = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "arbos_probe_phase").map { unsafeBitCast($0, to: ProbeFn.self) }
func warmupProbe(_ p: Int32) { probeFn?(p) }
final class M: ObservableObject { @Published var x = 0 }
enum FirstOpenWarmup {
  static func go(_ m: M) {
    m.objectWillChange.send()
    FileManager.default.createFile(atPath: NSTemporaryDirectory() + "probe-selftest-1.txt", contents: Data("a".utf8))
    _ = fopen(NSTemporaryDirectory() + "probe-selftest-2.txt", "w").map { fclose($0) }
    _ = SecItemCopyMatching([kSecClass: kSecClassGenericPassword] as CFDictionary, nil)
    NotificationCenter.default.addObserver(forName: .init("x"), object: nil, queue: nil) { _ in }
    UserDefaults.standard.set(2, forKey: "k2")
    Task { print("t") }
    URLSession.shared.dataTask(with: URL(string: "http://127.0.0.1:9/")!).resume()
  }
}
let m = M()
print("probe present:", probeFn != nil)
m.objectWillChange.send()                 // before
UserDefaults.standard.set(1, forKey: "k") // before
warmupProbe(1); FirstOpenWarmup.go(m); warmupProbe(2)
let t = Thread { warmupProbe(3); _ = fopen("/etc/hosts", "r").map { fclose($0) }; warmupProbe(4) }
t.start()
sleep(1); m.objectWillChange.send()       // after
sleep(4)
print("done")
