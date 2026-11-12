// Notification Service Extension: an ARBOS_MESSAGE push becomes a
// communication notification with the sender's picture (see
// Shared/MessageNotification.swift for the payload and the rendering).
// Anything else passes through untouched.
//
// An alert for an end-to-end encrypted message says "New message" and carries the event as
// the homeserver holds it; this reads it with the device's own keys and shows the words
// (phase 5). Whatever goes wrong, the alert is delivered as the relay wrote it.

import ArbosFFI
import os
import UserNotifications

private let log = Logger(subsystem: "com.unarbos.arbos.ios", category: "preview")

final class NotificationService: UNNotificationServiceExtension {
    private var handler: ((UNNotificationContent) -> Void)?
    private var content: UNMutableNotificationContent?

    override func didReceive(_ request: UNNotificationRequest, withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void) {
        handler = contentHandler
        let mutable = (request.content.mutableCopy() as? UNMutableNotificationContent) ?? UNMutableNotificationContent()
        content = mutable
        guard let push = MessagePush(userInfo: request.content.userInfo) else {
            contentHandler(mutable)
            return
        }
        Task { @MainActor in
            var push = push
            if let words = await Self.unsealed(push) {
                push.show(words)
                mutable.body = push.body
            }
            let updated = await push.makeContent(base: mutable)
            self.finish(updated)
        }
    }

    /// Memory that must be left before the keys are opened: the system stops the extension
    /// at about 24 MB, and a stopped extension delivers the alert only as it arrived.
    private static let headroom = 8 * 1024 * 1024

    /// The words of the encrypted message `push` announces, read with this device's keys. Nil
    /// when the alert carries no event, this build has no App Group, the device has no key
    /// for the message (or does not trust who sent it), the message is not from the sender
    /// the alert names, or too little memory is left to try.
    private static func unsealed(_ push: MessagePush) async -> String? {
        guard let sealed = push.sealed, let root = PreviewVault.storeRoot, let device = PreviewVault.load() else { return nil }
        // 0 where the system sets no limit (the simulator).
        let free = os_proc_available_memory()
        guard free == 0 || free > headroom else {
            log.notice("preview skipped: \(free) bytes left")
            return nil
        }
        let crypto = MatrixCrypto(storeDir: root.appendingPathComponent(device.deviceId, isDirectory: true).path,
                                  storeKey: device.storeKey)
        let preview = await Task.detached {
            matrixPreview(crypto: crypto, userId: device.userId, deviceId: device.deviceId,
                          roomId: sealed.roomId, eventJson: sealed.event)
        }.value
        log.notice("preview \(preview == nil ? "not read" : "read", privacy: .public), footprint \(footprint() >> 10) kB, peak \(footprint(peak: true) >> 10) kB")
        guard let preview, preview.sender.caseInsensitiveCompare("user:\(push.senderId)") == .orderedSame else { return nil }
        return preview.text
    }

    /// This process's memory as the system's limit counts it, now or at its highest.
    private static func footprint(peak: Bool = false) -> Int64 {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
        let ok = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        guard ok == KERN_SUCCESS else { return 0 }
        return peak ? info.ledger_phys_footprint_peak : Int64(info.phys_footprint)
    }

    /// Out of time: deliver what we have (title/body are already set by the relay).
    override func serviceExtensionTimeWillExpire() {
        if let content { finish(content) }
    }

    private func finish(_ c: UNNotificationContent) {
        guard let h = handler else { return }
        handler = nil
        h(c)
    }
}
