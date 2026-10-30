// Debug-only trace of one send: the hops between the composer tap and the own
// bubble on screen, each stamped in ms from the tap (iphone-group-send-instant,
// 2026-10-25). On with `ARBOS_TRACE_SENDS=1`; read with
//
//   log show --predicate 'subsystem == "com.unarbos.arbos.ios" AND category == "send"'
//
// Hops: tap → doc-written → commit → doc-change (branch) → snapshot-request →
// snapshot-read → messages-has-row → rows-has-row → row-in-tree → at-bottom,
// plus relay-ack when the relay confirms the push. Marks repeat per send; a
// mark that never shows for a send is a hop that did not run.

#if DEBUG
import Foundation
import os

@MainActor
enum SendTrace {
    private static let log = Logger(subsystem: "com.unarbos.arbos.ios", category: "send")
    private static var t0: CFAbsoluteTime = 0
    private static var chatId: String?
    private(set) static var messageId: String?
    private static var seen: Set<String> = []
    private static var sends = 0

    /// Read once: the body marks ask on every pass and must not cost an environment lookup each.
    static let enabled: Bool = DebugHooks.traceSends

    /// Composer tap. `rows` = transcript rows at that moment; `caughtUp` = the room is live.
    static func begin(chatId: String, rows: Int, caughtUp: Bool) {
        guard enabled else { return }
        sends += 1
        t0 = CFAbsoluteTimeGetCurrent()
        self.chatId = chatId
        messageId = nil
        seen = []
        log.notice("send #\(sends) tap chat=\(chatId, privacy: .public) rows=\(rows) caughtUp=\(caughtUp)")
    }

    /// The row's id, once the writer minted it.
    static func message(_ id: String, chatId: String) {
        guard enabled, chatId == self.chatId else { return }
        messageId = id
        mark("doc-written", detail: "id=\(id)")
    }

    static func traces(_ chatId: String) -> Bool {
        enabled && t0 > 0 && chatId == self.chatId
    }

    /// Within ten seconds of a tap (scroll geometry is logged only then).
    static var recent: Bool {
        enabled && t0 > 0 && CFAbsoluteTimeGetCurrent() - t0 < 10
    }

    /// One hop. `once`: the first time per send only (later repeats of the same hop are noise).
    static func mark(_ what: String, detail: String = "", once: Bool = true) {
        guard enabled, t0 > 0 else { return }
        if once, !seen.insert(what).inserted { return }
        let ms = (CFAbsoluteTimeGetCurrent() - t0) * 1000
        log.notice("send #\(sends) +\(ms, format: .fixed(precision: 1))ms \(what, privacy: .public) \(detail, privacy: .public)")
    }

    /// A hop that applies only when the list it saw holds the traced row.
    static func markIfPresent(_ what: String, ids: @autoclosure () -> [String], detail: String = "") {
        guard enabled, let id = messageId, !seen.contains(what), ids().contains(id) else { return }
        mark(what, detail: detail)
    }
}
#endif
