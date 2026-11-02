import Foundation
typealias ProbeFn = @convention(c) (Int32) -> Void
let probeFn: ProbeFn? = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "arbos_probe_phase").map { unsafeBitCast($0, to: ProbeFn.self) }
let s = Darwin.socket(AF_INET, SOCK_DGRAM, 0)
var a = sockaddr_in(); a.sin_family = sa_family_t(AF_INET); a.sin_port = in_port_t(53).bigEndian; a.sin_addr.s_addr = inet_addr("127.0.0.1")
_ = withUnsafePointer(to: &a) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(s, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) } }
var r = URLRequest(url: URL(string: "http://127.0.0.1:9/x?y=1")!); r.httpMethod = "POST"; r.setValue("Bearer t", forHTTPHeaderField: "Authorization")
URLSession.shared.dataTask(with: r).resume()
sleep(1)
// connect() records are logged only in the dump, which runs 3 s after phase 4.
for p in Int32(1)...4 { probeFn?(p) }
sleep(4); print("done")
