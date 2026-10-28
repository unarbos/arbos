// Opt-in performance diagnostics (iphone-scroll-perf, 2026-10-27). With Settings › About ›
// "Share performance diagnostics" on (off by default), the app subscribes to MetricKit and
// forwards what iOS hands it — once a day an `MXMetricPayload` (scroll hitch time ratio,
// hang-time histogram, launch and resume times, CPU, memory, exits) and, as they happen,
// `MXDiagnosticPayload`s (hangs, crashes, with call trees) — to the relay's `POST /perf/report`
// as the payload's own JSON. Nothing about the user or the device identity goes with it:
// platform, app version and build, OS version and device model only. MetricKit does not
// deliver on the simulator; this is for TestFlight phones.
//
// Why: there is no real-iPhone test rig, and Jacob's phone is not one. This is how his
// phone's own scroll / hang / launch numbers reach the loop from a TestFlight build.

import Foundation
import MetricKit
import UIKit

@MainActor
final class PerfDiagnostics: NSObject, MXMetricManagerSubscriber {
    static let shared = PerfDiagnostics()
    static let defaultsKey = "sh.arbos.perfDiagnostics.enabled"

    private weak var model: AppModel?
    private var subscribed = false
    /// Payloads MetricKit delivered before sign-in / while offline; sent on the next chance.
    private var queue: [(kind: String, payload: Any)] = []

    static var enabled: Bool {
        get { UserDefaults.standard.bool(forKey: defaultsKey) }
        set {
            UserDefaults.standard.set(newValue, forKey: defaultsKey)
            shared.apply()
        }
    }

    func start(model: AppModel) {
        self.model = model
        apply()
    }

    private func apply() {
        let wanted = Self.enabled
        if wanted, !subscribed {
            MXMetricManager.shared.add(self)
            subscribed = true
            // Payloads from the last 24 h that arrived while nothing listened.
            receive(MXMetricManager.shared.pastPayloads.map { ("metrics", $0.jsonRepresentation()) })
            receive(MXMetricManager.shared.pastDiagnosticPayloads.map { ("diagnostics", $0.jsonRepresentation()) })
        } else if !wanted, subscribed {
            MXMetricManager.shared.remove(self)
            subscribed = false
            queue.removeAll()
        }
    }

    // MARK: MXMetricManagerSubscriber (called on a background queue)

    nonisolated func didReceive(_ payloads: [MXMetricPayload]) {
        let items = payloads.map { ("metrics", $0.jsonRepresentation()) }
        Task { @MainActor in self.receive(items) }
    }

    nonisolated func didReceive(_ payloads: [MXDiagnosticPayload]) {
        let items = payloads.map { ("diagnostics", $0.jsonRepresentation()) }
        Task { @MainActor in self.receive(items) }
    }

    private func receive(_ items: [(String, Data)]) {
        guard Self.enabled else { return }
        for (kind, data) in items {
            guard let object = try? JSONSerialization.jsonObject(with: data), object is [String: Any] else { continue }
            queue.append((kind, object))
        }
        Task { await flush() }
    }

    /// The envelope around a payload: what the relay stores beside it. No identity.
    static func envelope(kind: String, payload: Any) -> [String: Any] {
        let info = Bundle.main.infoDictionary
        return [
            "kind": kind,
            "platform": "ios",
            "app_version": info?["CFBundleShortVersionString"] as? String ?? "",
            "build": info?["CFBundleVersion"] as? String ?? "",
            "os": "iOS \(UIDevice.current.systemVersion)",
            "device_model": Self.machine,
            "payload": payload,
        ]
    }

    /// `iPhone17,1` and the like (the model identifier, not a name).
    static var machine: String {
        var info = utsname()
        uname(&info)
        return withUnsafePointer(to: &info.machine) {
            $0.withMemoryRebound(to: CChar.self, capacity: Int(_SYS_NAMELEN)) { String(cString: $0) }
        }
    }

    private func flush() async {
        guard let model, model.isPaired, Self.enabled else { return }
        while !queue.isEmpty {
            let next = queue[0]
            do {
                try await model.sendPerfReport(Self.envelope(kind: next.kind, payload: next.payload))
                queue.removeFirst()
            } catch {
                // Offline or the relay is down: keep the queue, try again on the next payload.
                return
            }
        }
    }
}
