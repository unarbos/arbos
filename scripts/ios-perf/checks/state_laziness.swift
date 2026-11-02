import SwiftUI
@MainActor var log: [String] = []
final class P: ObservableObject { init(_ t: String) { MainActor.assumeIsolated { log.append("init \(t)") } } }
struct V: View {
    let a: Int
    @State private var s = P("State")
    @StateObject private var o = P("StateObject")
    @ObservedObject private var ob = P("ObservedObject")
    @State private var d = Date()
    var body: some View { Text("x") }
}
MainActor.assumeIsolated {
    let v = V(a: 1)
    for c in Mirror(reflecting: v).children { _ = type(of: c.value) }
    log.append("constructed + mirrored")
    print(log)
}
