import SwiftUI
import AppKit

@MainActor var log: [String] = []
struct Probe: View {
    let tag: String
    @State var n = 0
    var body: some View {
        let _ = log.append("\(tag) body")
        VStack { Text("hello \(n)"); Color.red.frame(width: 10, height: 10) }
            .onAppear { log.append("\(tag) onAppear") }
            .task { log.append("\(tag) task") }
            .onChange(of: n, initial: true) { _, _ in log.append("\(tag) onChange-initial") }
    }
}

@MainActor func run() {
    _ = NSApplication.shared
    let hc = NSHostingController(rootView: Probe(tag: "hostingNoWindow"))
    let s = hc.sizeThatFits(in: CGSize(width: 400, height: 800))
    log.append("sized \(s)")
    let hv = NSHostingView(rootView: Probe(tag: "hostingViewLayout"))
    hv.frame = CGRect(x: 0, y: 0, width: 400, height: 800)
    hv.layoutSubtreeIfNeeded()
    _ = hv.fittingSize
    let r = ImageRenderer(content: Probe(tag: "imageRenderer"))
    _ = r.cgImage
}
MainActor.assumeIsolated { run() }
let end = Date().addingTimeInterval(1.5)
while Date() < end { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
MainActor.assumeIsolated { for l in log { print(l) } }
