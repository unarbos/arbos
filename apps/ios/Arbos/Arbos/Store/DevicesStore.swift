// Settings → Devices (Jacob, 2026-10-23): the account's signed-in devices from
// `GET /auth/devices`, and `DELETE /auth/devices/{id}` to sign another one out.
// List + revoke only; second-device approval (RFC 8628) stays deferred. The
// Mac twin is `ui/devices.rs` + `profiles/me.rs`.

import Foundation

/// One signed-in device of my account.
struct AuthDevice: Identifiable, Equatable {
    var id: String
    var name: String
    /// `github` (a GitHub sign-in), `paircode`, `reader` (read-only, Grok Bot) …
    var kind: String
    var createdAt: Int64
    var lastSeenAt: Int64?
    /// Set once the device signed out or was revoked: its session ended, so
    /// it is not a device I am logged in on and the list leaves it out.
    var revokedAt: Int64? = nil
    /// The device that made the request — this iPhone.
    var current: Bool

    static func parse(_ v: Any?) -> AuthDevice? {
        guard let d = v as? [String: Any], let id = d["id"] as? String else { return nil }
        return AuthDevice(
            id: id,
            name: (d["name"] as? String) ?? "",
            kind: (d["kind"] as? String) ?? "",
            createdAt: (d["created_at"] as? NSNumber)?.int64Value ?? 0,
            lastSeenAt: (d["last_seen_at"] as? NSNumber)?.int64Value,
            revokedAt: (d["revoked_at"] as? NSNumber)?.int64Value,
            current: (d["current"] as? Bool) ?? false
        )
    }

    /// Whether this row is still a session (nothing signed it out).
    var isLive: Bool { revokedAt == nil }

    /// What Settings → Devices shows: live rows only, this iPhone first, the
    /// rest in the relay's order (oldest sign-in first). A tester who had
    /// signed four Macs out still saw "9 devices" before this filter.
    static func visible(_ list: [AuthDevice]) -> [AuthDevice] {
        let live = list.filter(\.isLive)
        return live.filter(\.current) + live.filter { !$0.current }
    }

    /// The row's name, never empty.
    var displayName: String {
        let t = name.trimmingCharacters(in: .whitespaces)
        return t.isEmpty ? "Unnamed device" : t
    }

    /// The tile glyph (SF Symbol): a reader is a bot, a phone or iPad a phone,
    /// a Mac a laptop.
    var glyph: String {
        let n = name.lowercased()
        if kind == "reader" || n.contains("bot") { return "cpu" }
        if n.contains("iphone") || n.contains("ipad") || n.contains("phone") { return "iphone" }
        if n.contains("imac") || n.contains("mac mini") || n.contains("studio") { return "desktopcomputer" }
        return "laptopcomputer"
    }

    /// `kind` as a small tag: readers say "read-only", pairing-code devices
    /// "pair code"; a GitHub sign-in needs none.
    var kindTag: String? {
        switch kind {
        case "reader": return "read-only"
        case "paircode": return "pair code"
        default: return nil
        }
    }

    /// "Active now · added Oct 11" / "2 h ago · added Oct 14".
    func subtitle(now: Int64 = Int64(Date().timeIntervalSince1970 * 1000)) -> String {
        "\(AuthDevice.seenLabel(now: now, lastSeenAt: lastSeenAt)) · added \(AuthDevice.dateLabel(createdAt))"
    }

    /// "Active now" within two minutes, then minutes / hours / days, then the date.
    static func seenLabel(now: Int64, lastSeenAt: Int64?) -> String {
        guard let seen = lastSeenAt else { return "Never seen" }
        let min = max(now - seen, 0) / 60_000
        if min < 2 { return "Active now" }
        if min < 60 { return "\(min) min ago" }
        if min < 24 * 60 { return "\(min / 60) h ago" }
        if min < 14 * 24 * 60 { return "\(min / (24 * 60)) d ago" }
        return dateLabel(seen)
    }

    /// "Oct 11" this year, "Oct 11, 2025" otherwise.
    static func dateLabel(_ ms: Int64) -> String {
        let date = Date(timeIntervalSince1970: Double(ms) / 1000)
        let f = DateFormatter()
        let sameYear = Calendar.current.component(.year, from: date) == Calendar.current.component(.year, from: Date())
        f.setLocalizedDateFormatFromTemplate(sameYear ? "MMM d" : "MMM d yyyy")
        return f.string(from: date)
    }

    /// The confirm line under a Sign out.
    static func confirmQuestion(_ name: String) -> String {
        "Sign out \(name)? It will need to sign in again."
    }

    /// Drop `id` unless it is the current device — the list never loses this
    /// iPhone through a revoke.
    @discardableResult
    static func removeOther(_ list: inout [AuthDevice], id: String) -> Bool {
        let before = list.count
        list.removeAll { $0.id == id && !$0.current }
        return list.count != before
    }

    /// Three devices for the fixture: this iPhone (current), a MacBook seen an
    /// hour ago, and a read-only Grok Bot reader from last week.
    static func fixture(now: Int64 = Int64(Date().timeIntervalSince1970 * 1000)) -> [AuthDevice] {
        [
            AuthDevice(id: "dev-fx-iphone", name: "iPhone", kind: "github", createdAt: now - 9 * 24 * 3_600_000, lastSeenAt: now - 20_000, current: true),
            AuthDevice(id: "dev-fx-mac", name: "Jacob's MacBook Pro", kind: "github", createdAt: now - 12 * 24 * 3_600_000, lastSeenAt: now - 3_600_000, current: false),
            AuthDevice(id: "dev-fx-grokbot", name: "GrokBot-Arbos", kind: "reader", createdAt: now - 6 * 24 * 3_600_000, lastSeenAt: now - 3 * 24 * 3_600_000, current: false),
        ]
    }
}

extension RelayAPI {
    func devices() async throws -> [AuthDevice] {
        guard let list = try await json("GET", "/auth/devices") as? [Any] else {
            throw RelayError(status: 0, message: "Bad devices reply")
        }
        return list.compactMap(AuthDevice.parse)
    }

    func revokeDevice(_ id: String) async throws {
        _ = try await request("DELETE", "/auth/devices/\(id)")
    }
}

extension AppModel {
    /// Settings → Devices opened: read the list (the fixture seeds three).
    func refreshDevices() async {
        if isFixture {
            devices = AuthDevice.visible(AuthDevice.fixture())
            devicesLoaded = true
            return
        }
        guard credentials != nil else { return }
        do {
            devices = try await AuthDevice.visible(relay.devices())
            devicesError = nil
        } catch {
            devicesError = UserError.classify("load your devices", error)
        }
        devicesLoaded = true
    }

    /// Sign another device out. The current device never goes through here
    /// (its row offers Sign out instead), so you are never stranded.
    func revokeDevice(_ id: String) {
        guard AuthDevice.removeOther(&devices, id: id) else { return }
        guard !isFixture else { return }
        Task {
            do {
                try await relay.revokeDevice(id)
            } catch {
                fail("sign out that device", error)
            }
            await refreshDevices()
        }
    }
}
