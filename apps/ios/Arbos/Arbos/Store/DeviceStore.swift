// On-device store (#929): one SQLite file per (relay, signed-in device) under
// Application Support. It holds the last registry / people / account JSON and
// a Loro snapshot + relay cursor per room, so a cold launch paints the home
// list and every transcript from disk and the rooms only ask the relay for
// rows after the saved cursor.
//
// Rules: cursor and snapshot are written in one row (never out of step); a
// schema bump drops everything and refetches; sign-out deletes the file;
// per-doc and total caps with least-recently-opened eviction.

import Foundation
import SQLite3
import os

final class DeviceStore {
    static let schemaVersion: Int32 = 1
    /// A single chat snapshot above this is not kept (it streams from the relay instead).
    static let perDocCapBytes = 8_000_000
    /// Everything on disk for one account.
    static let totalCapBytes: Int64 = 250_000_000

    struct SavedDoc {
        let key: String
        let projectId: String
        let roomChat: String
        let cursor: Int64
        let snapshot: Data
        let openedAt: Int64
    }

    struct Usage: Equatable {
        var bytes: Int64 = 0
        var docs: Int = 0
    }

    private static let log = Logger(subsystem: "com.unarbos.arbos.ios", category: "store")
    private static let SQLITE_TRANSIENT = unsafeBitCast(-1, to: sqlite3_destructor_type.self)

    let url: URL
    private var db: OpaquePointer?
    /// Every statement runs here; reads at launch use `sync`, writes `async`.
    private let queue = DispatchQueue(label: "sh.arbos.ios.store", qos: .utility)

    // MARK: Open / wipe

    static func directory() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? FileManager.default.temporaryDirectory
        return base.appendingPathComponent("Arbos", isDirectory: true)
    }

    static func fileURL(relayUrl: String, account: String) -> URL {
        var slug = ""
        for c in (relayUrl + "|" + account).unicodeScalars {
            slug.append(CharacterSet.alphanumerics.contains(c) ? Character(c) : "_")
        }
        // Keep the name readable but bounded.
        let name = slug.count > 80 ? String(slug.suffix(80)) : slug
        return directory().appendingPathComponent("store-\(name).sqlite")
    }

    /// Keep the whole store folder out of iTunes/iCloud backups. Excluding the `.sqlite`
    /// file alone left `-wal` and `-shm` behind, and in WAL mode the newest chat content is
    /// in the `-wal` — so an unencrypted backup still carried recent messages. The relay has
    /// everything in here anyway, so nothing is lost by leaving it out.
    static func excludeFromBackup() {
        var dir = directory()
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? dir.setResourceValues(values)
    }

    static func open(relayUrl: String, account: String) -> DeviceStore? {
        let url = fileURL(relayUrl: relayUrl, account: account)
        try? FileManager.default.createDirectory(at: directory(), withIntermediateDirectories: true)
        excludeFromBackup()
        if let s = DeviceStore(url: url) { return s }
        // Unreadable or corrupt: start over (the relay has everything).
        log.error("store: could not open \(url.lastPathComponent, privacy: .public); recreating")
        DeviceStore.remove(url)
        return DeviceStore(url: url)
    }

    static func wipe(relayUrl: String, account: String) {
        remove(fileURL(relayUrl: relayUrl, account: account))
    }

    private static func remove(_ url: URL) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            try? FileManager.default.removeItem(at: URL(fileURLWithPath: url.path + suffix))
        }
    }

    private init?(url: URL) {
        self.url = url
        var handle: OpaquePointer?
        let flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX
        guard sqlite3_open_v2(url.path, &handle, flags, nil) == SQLITE_OK, let handle else {
            if let handle { sqlite3_close(handle) }
            return nil
        }
        db = handle
        var excluded = URLResourceValues()
        excluded.isExcludedFromBackup = true
        var u = url
        try? u.setResourceValues(excluded)
        DeviceStore.excludeFromBackup()
        guard migrate() else {
            sqlite3_close(handle)
            db = nil
            return nil
        }
    }

    deinit {
        if let db { sqlite3_close(db) }
    }

    /// Create the tables; a version mismatch drops them (drop-and-refetch).
    private func migrate() -> Bool {
        guard exec("PRAGMA journal_mode=WAL") && exec("PRAGMA synchronous=NORMAL") else { return false }
        let version = scalarInt("PRAGMA user_version") ?? 0
        if version != 0 && version != Int64(DeviceStore.schemaVersion) {
            DeviceStore.log.notice("store: schema \(version) → \(DeviceStore.schemaVersion); dropping")
            guard exec("DROP TABLE IF EXISTS kv"), exec("DROP TABLE IF EXISTS docs"), exec("DROP TABLE IF EXISTS pending") else { return false }
        }
        let ok = exec("""
            CREATE TABLE IF NOT EXISTS kv (
              k TEXT PRIMARY KEY, v BLOB NOT NULL, updated_at INTEGER NOT NULL)
            """) && exec("""
            CREATE TABLE IF NOT EXISTS docs (
              key TEXT PRIMARY KEY, project_id TEXT NOT NULL, room_chat TEXT NOT NULL,
              cursor INTEGER NOT NULL, snapshot BLOB NOT NULL, bytes INTEGER NOT NULL,
              updated_at INTEGER NOT NULL, opened_at INTEGER NOT NULL)
            """) && exec("""
            CREATE TABLE IF NOT EXISTS pending (
              key TEXT NOT NULL, batch_id TEXT NOT NULL, bytes BLOB NOT NULL, seq INTEGER NOT NULL,
              PRIMARY KEY (key, batch_id))
            """) && exec("PRAGMA user_version=\(DeviceStore.schemaVersion)")
        return ok
    }

    // MARK: Key/value (registry, people, account JSON)

    func put(_ key: String, _ data: Data) {
        queue.async { [self] in
            run("INSERT OR REPLACE INTO kv (k, v, updated_at) VALUES (?, ?, ?)") { s in
                sqlite3_bind_text(s, 1, key, -1, DeviceStore.SQLITE_TRANSIENT)
                bindBlob(s, 2, data)
                sqlite3_bind_int64(s, 3, nowMs())
            }
        }
    }

    func get(_ key: String) -> Data? {
        queue.sync { [self] in
            var out: Data?
            query("SELECT v FROM kv WHERE k = ?", bind: { s in
                sqlite3_bind_text(s, 1, key, -1, DeviceStore.SQLITE_TRANSIENT)
            }) { s in
                out = columnBlob(s, 0)
                return false
            }
            return out
        }
    }

    func remove(_ key: String) {
        queue.async { [self] in
            run("DELETE FROM kv WHERE k = ?") { s in
                sqlite3_bind_text(s, 1, key, -1, DeviceStore.SQLITE_TRANSIENT)
            }
        }
    }

    // MARK: Docs (Loro snapshot + relay cursor per room)

    /// Snapshot and cursor land in one statement so they can never disagree.
    /// A snapshot over the per-doc cap replaces nothing and drops the old row.
    func saveDoc(key: String, projectId: String, roomChat: String, cursor: Int64, snapshot: Data) {
        queue.async { [self] in
            if snapshot.count > DeviceStore.perDocCapBytes {
                run("DELETE FROM docs WHERE key = ?") { s in
                    sqlite3_bind_text(s, 1, key, -1, DeviceStore.SQLITE_TRANSIENT)
                }
                return
            }
            let now = nowMs()
            run("""
                INSERT INTO docs (key, project_id, room_chat, cursor, snapshot, bytes, updated_at, opened_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(key) DO UPDATE SET cursor = excluded.cursor, snapshot = excluded.snapshot,
                  bytes = excluded.bytes, updated_at = excluded.updated_at
                """) { s in
                sqlite3_bind_text(s, 1, key, -1, DeviceStore.SQLITE_TRANSIENT)
                sqlite3_bind_text(s, 2, projectId, -1, DeviceStore.SQLITE_TRANSIENT)
                sqlite3_bind_text(s, 3, roomChat, -1, DeviceStore.SQLITE_TRANSIENT)
                sqlite3_bind_int64(s, 4, cursor)
                bindBlob(s, 5, snapshot)
                sqlite3_bind_int64(s, 6, Int64(snapshot.count))
                sqlite3_bind_int64(s, 7, now)
                sqlite3_bind_int64(s, 8, now)
            }
        }
    }

    /// Every saved room, most recently opened first.
    func loadDocs() -> [SavedDoc] {
        queue.sync { [self] in
            var out: [SavedDoc] = []
            query("SELECT key, project_id, room_chat, cursor, snapshot, opened_at FROM docs ORDER BY opened_at DESC") { s in
                guard let key = columnText(s, 0), let project = columnText(s, 1), let room = columnText(s, 2),
                      let snapshot = columnBlob(s, 4) else { return true }
                out.append(SavedDoc(key: key, projectId: project, roomChat: room, cursor: sqlite3_column_int64(s, 3),
                                    snapshot: snapshot, openedAt: sqlite3_column_int64(s, 5)))
                return true
            }
            return out
        }
    }

    /// The user opened this chat: it is the last to be evicted.
    func touchDoc(_ key: String) {
        queue.async { [self] in
            run("UPDATE docs SET opened_at = ? WHERE key = ?") { s in
                sqlite3_bind_int64(s, 1, nowMs())
                sqlite3_bind_text(s, 2, key, -1, DeviceStore.SQLITE_TRANSIENT)
            }
        }
    }

    func deleteDoc(_ key: String) {
        queue.async { [self] in
            run("DELETE FROM docs WHERE key = ?") { s in
                sqlite3_bind_text(s, 1, key, -1, DeviceStore.SQLITE_TRANSIENT)
            }
            run("DELETE FROM pending WHERE key = ?") { s in
                sqlite3_bind_text(s, 1, key, -1, DeviceStore.SQLITE_TRANSIENT)
            }
        }
    }

    // MARK: Pending pushes (rows the relay has not ACKed yet)

    /// Replace the unacked batches of one room (order kept by `seq`).
    func savePending(key: String, batches: [(String, Data)]) {
        queue.async { [self] in
            run("DELETE FROM pending WHERE key = ?") { s in
                sqlite3_bind_text(s, 1, key, -1, DeviceStore.SQLITE_TRANSIENT)
            }
            for (i, (batchId, bytes)) in batches.enumerated() {
                run("INSERT OR REPLACE INTO pending (key, batch_id, bytes, seq) VALUES (?, ?, ?, ?)") { s in
                    sqlite3_bind_text(s, 1, key, -1, DeviceStore.SQLITE_TRANSIENT)
                    sqlite3_bind_text(s, 2, batchId, -1, DeviceStore.SQLITE_TRANSIENT)
                    bindBlob(s, 3, bytes)
                    sqlite3_bind_int64(s, 4, Int64(i))
                }
            }
        }
    }

    func loadPending() -> [String: [(String, Data)]] {
        queue.sync { [self] in
            var out: [String: [(String, Data)]] = [:]
            query("SELECT key, batch_id, bytes FROM pending ORDER BY key, seq") { s in
                if let key = columnText(s, 0), let batch = columnText(s, 1), let bytes = columnBlob(s, 2) {
                    out[key, default: []].append((batch, bytes))
                }
                return true
            }
            return out
        }
    }

    // MARK: Caps and usage

    func usage() -> Usage {
        queue.sync { [self] in
            var u = Usage()
            // `proj:` rows are project docs, not chats the user counts.
            query("SELECT COALESCE(SUM(bytes), 0), COUNT(CASE WHEN key NOT LIKE 'proj:%' THEN 1 END) FROM docs") { s in
                u.bytes = sqlite3_column_int64(s, 0)
                u.docs = Int(sqlite3_column_int64(s, 1))
                return false
            }
            query("SELECT COALESCE(SUM(LENGTH(v)), 0) FROM kv") { s in
                u.bytes += sqlite3_column_int64(s, 0)
                return false
            }
            return u
        }
    }

    /// Drop the least recently opened docs until the total fits the cap.
    func enforceCaps() {
        queue.async { [self] in
            var total: Int64 = 0
            query("SELECT COALESCE(SUM(bytes), 0) FROM docs") { s in
                total = sqlite3_column_int64(s, 0)
                return false
            }
            guard total > DeviceStore.totalCapBytes else { return }
            var victims: [(String, Int64)] = []
            query("SELECT key, bytes FROM docs ORDER BY opened_at ASC, updated_at ASC") { s in
                if let key = columnText(s, 0) { victims.append((key, sqlite3_column_int64(s, 1))) }
                return true
            }
            for (key, bytes) in victims where total > DeviceStore.totalCapBytes {
                run("DELETE FROM docs WHERE key = ?") { s in
                    sqlite3_bind_text(s, 1, key, -1, DeviceStore.SQLITE_TRANSIENT)
                }
                total -= bytes
            }
            DeviceStore.log.notice("store: evicted down to \(total) bytes")
        }
    }

    /// Flush queued writes and close; the file is deleted when `wipe` is set.
    func close(wipe: Bool) {
        queue.sync { [self] in
            if let db {
                sqlite3_close(db)
                self.db = nil
            }
            if wipe { DeviceStore.remove(url) }
        }
    }

    /// Block until every queued write has landed (scene → background).
    func flush() {
        queue.sync {}
    }

    // MARK: SQLite plumbing

    private func exec(_ sql: String) -> Bool {
        guard let db else { return false }
        var err: UnsafeMutablePointer<CChar>?
        let rc = sqlite3_exec(db, sql, nil, nil, &err)
        if rc != SQLITE_OK {
            let text = err.map { String(cString: $0) } ?? "rc \(rc)"
            DeviceStore.log.error("store: \(sql.prefix(40), privacy: .public): \(text, privacy: .public)")
            sqlite3_free(err)
            return false
        }
        return true
    }

    private func scalarInt(_ sql: String) -> Int64? {
        var out: Int64?
        query(sql) { s in
            out = sqlite3_column_int64(s, 0)
            return false
        }
        return out
    }

    private func run(_ sql: String, bind: (OpaquePointer) -> Void) {
        guard let db else { return }
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK, let stmt else {
            DeviceStore.log.error("store: prepare failed: \(String(cString: sqlite3_errmsg(db)), privacy: .public)")
            return
        }
        defer { sqlite3_finalize(stmt) }
        bind(stmt)
        let rc = sqlite3_step(stmt)
        if rc != SQLITE_DONE && rc != SQLITE_ROW {
            DeviceStore.log.error("store: step failed: \(String(cString: sqlite3_errmsg(db)), privacy: .public)")
        }
    }

    /// `row` returns false to stop early.
    private func query(_ sql: String, bind: ((OpaquePointer) -> Void)? = nil, row: (OpaquePointer) -> Bool) {
        guard let db else { return }
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK, let stmt else {
            DeviceStore.log.error("store: prepare failed: \(String(cString: sqlite3_errmsg(db)), privacy: .public)")
            return
        }
        defer { sqlite3_finalize(stmt) }
        bind?(stmt)
        while sqlite3_step(stmt) == SQLITE_ROW {
            if !row(stmt) { break }
        }
    }

    private func bindBlob(_ s: OpaquePointer, _ ix: Int32, _ data: Data) {
        data.withUnsafeBytes { raw in
            _ = sqlite3_bind_blob(s, ix, raw.baseAddress, Int32(raw.count), DeviceStore.SQLITE_TRANSIENT)
        }
    }

    private func columnBlob(_ s: OpaquePointer, _ ix: Int32) -> Data? {
        let n = Int(sqlite3_column_bytes(s, ix))
        guard n > 0, let p = sqlite3_column_blob(s, ix) else { return Data() }
        return Data(bytes: p, count: n)
    }

    private func columnText(_ s: OpaquePointer, _ ix: Int32) -> String? {
        sqlite3_column_text(s, ix).map { String(cString: $0) }
    }
}
