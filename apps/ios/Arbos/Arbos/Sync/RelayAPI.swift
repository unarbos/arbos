// HTTP side of `arbos-relay` (crdt-sync §9): pairing, registry, chats,
// models, wake. One bearer token per call, read from the credentials at
// call time so a re-pair takes effect at once.

import Foundation
import ArbosFFI

struct RelayError: Error, LocalizedError {
    var status: Int
    var message: String
    /// `error.type` from the self-serve relay (token_expired, credits_exhausted, …).
    var type: String? = nil
    /// `error.detail` when the relay sends one (the avatar routes: the provider's words).
    var detail: String? = nil

    var errorDescription: String? {
        message.isEmpty ? "HTTP \(status)" : "HTTP \(status): \(message)"
    }

    var isUnauthorized: Bool { status == 401 }

    /// Decode `ApiErrorDto` when the body is the current shape; plain text for older relays.
    static func from(status: Int, body: Data) -> RelayError {
        if let dto = try? JSONDecoder().decode(ApiErrorDto.self, from: body) {
            let detail = ((try? JSONSerialization.jsonObject(with: body)) as? [String: Any])
                .flatMap { $0["error"] as? [String: Any] }
                .flatMap { $0["detail"] as? String }
                .flatMap { $0.isEmpty ? nil : $0 }
            return RelayError(status: status, message: dto.error.message, type: dto.error.type, detail: detail)
        }
        let text = (String(data: body, encoding: .utf8) ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return RelayError(status: status, message: text)
    }
}

/// Accepts `http://`, `https://`, `ws://` or `wss://`; derives the other.
struct RelayEndpoint: Equatable {
    var httpUrl: String
    var wsUrl: String

    init(_ raw: String) {
        var url = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        while url.hasSuffix("/") { url.removeLast() }
        if url.isEmpty {
            httpUrl = ""
            wsUrl = ""
        } else if url.hasPrefix("ws://") {
            httpUrl = "http://" + url.dropFirst(5)
            wsUrl = url
        } else if url.hasPrefix("wss://") {
            httpUrl = "https://" + url.dropFirst(6)
            wsUrl = url
        } else if url.hasPrefix("https://") {
            httpUrl = url
            wsUrl = "wss://" + url.dropFirst(8)
        } else if url.hasPrefix("http://") {
            httpUrl = url
            wsUrl = "ws://" + url.dropFirst(7)
        } else {
            httpUrl = "https://" + url
            wsUrl = "wss://" + url
        }
    }
}

struct RegistryProject {
    var project: Arbos.Project
    var chats: [Arbos.Chat]
}

/// Outcome of `GET /projects` + per-project detail. `skippedIds` are projects
/// whose list row parsed but detail failed to parse/decode (not 403/404).
/// `incomplete` is true when a list item had no readable id.
struct RegistryResult {
    var projects: [RegistryProject]
    var skippedIds: Set<String>
    var incomplete: Bool
}

/// List rows from `GET /projects` may overwrite `owner` with a user-id string; that
/// fails `ProjectDto` decoding, so we only need the id to fetch the detail.
private struct ProjectListId: Decodable {
    var id: String
}

final class RelayAPI {
    let endpoint: RelayEndpoint
    var token: () -> String
    /// Called once on a 401 for an authenticated call; return true after a
    /// successful token refresh and the call is retried with the new token.
    var refresher: (() async -> Bool)?

    private let session: URLSession

    init(endpoint: RelayEndpoint, token: @escaping () -> String) {
        self.endpoint = endpoint
        self.token = token
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = 30
        config.waitsForConnectivity = false
        session = URLSession(configuration: config)
    }

    // MARK: Raw calls

    /// `ios/1.0.0 (202610161326)` — who placed a call, for the relay's call record.
    static var clientHeader: String {
        let v = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "?"
        let b = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "?"
        return "ios/\(v) (\(b))"
    }

    /// `timeout`: this one request's limit instead of the session's 30 s (an avatar generation
    /// gets 90 s so the relay's 75 s budget always answers first).
    func request(_ method: String, _ path: String, body: Any? = nil, auth: Bool = true, headers: [String: String] = [:], timeout: TimeInterval? = nil) async throws -> (Data, HTTPURLResponse) {
        do {
            return try await requestOnce(method, path, body: body, auth: auth, headers: headers, timeout: timeout)
        } catch let error as RelayError where error.isUnauthorized && auth {
            if let refresher, await refresher() {
                return try await requestOnce(method, path, body: body, auth: auth, headers: headers, timeout: timeout)
            }
            throw error
        }
    }

    private func requestOnce(_ method: String, _ path: String, body: Any?, auth: Bool, headers: [String: String] = [:], timeout: TimeInterval? = nil) async throws -> (Data, HTTPURLResponse) {
        guard let url = URL(string: endpoint.httpUrl + path) else {
            throw RelayError(status: 0, message: "Bad relay URL")
        }
        var req = URLRequest(url: url)
        req.httpMethod = method
        if let timeout { req.timeoutInterval = timeout }
        if auth {
            req.setValue("Bearer \(token())", forHTTPHeaderField: "Authorization")
        }
        // What runs here (`ios/1.0.0 (202610192230)`): the relay's "installed version" signal for
        // continuous delivery (GET /admin/installs), on every call.
        req.setValue(Self.clientHeader, forHTTPHeaderField: "X-Arbos-Client")
        for (k, v) in headers { req.setValue(v, forHTTPHeaderField: k) }
        if let body {
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
            req.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        let (data, response) = try await session.data(for: req)
        guard let http = response as? HTTPURLResponse else {
            throw RelayError(status: 0, message: "No HTTP response")
        }
        if http.statusCode >= 400 {
            throw RelayError.from(status: http.statusCode, body: data)
        }
        return (data, http)
    }

    // MARK: Emoji · stickers · GIFs picker (ARB-544)

    /// `GET /gif/search?q=` (or `/gif/trending` for an empty query): the relay's page, or why
    /// there is none. The relay talks to KLIPY; this app never holds the key.
    func gifSearch(query: String, pos: String?) async -> GifPage {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        var path: String
        if q.isEmpty {
            path = "/gif/trending"
            if let pos, !pos.isEmpty, let enc = pos.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) {
                path += "?pos=\(enc)"
            }
        } else {
            guard let enc = q.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) else { return .failed }
            path = "/gif/search?q=\(enc)"
            if let pos, !pos.isEmpty, let penc = pos.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) {
                path += "&pos=\(penc)"
            }
        }
        do {
            let (data, _) = try await request("GET", path)
            let page = try JSONDecoder().decode(GifPageDto.self, from: data)
            return .items(page.items, next: page.next)
        } catch let error as RelayError {
            if error.type == "gif_unavailable" || error.status == 503 { return .unavailable }
            if error.type == "gif_busy" || error.status == 429 { return .busy }
            return .failed
        } catch {
            return .failed
        }
    }

    /// `POST /gif/share?id=&q=` — the provider's ranking signal; never an error for the app.
    func gifShare(id: String, query: String) async {
        guard let i = id.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed),
              let q = query.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) else { return }
        _ = try? await request("POST", "/gif/share?id=\(i)&q=\(q)")
    }

    /// `GET /stickers/packs` — the relay's sticker-pack manifest; `nil` when it could not be read.
    func stickerPacks() async -> [StickerPack]? {
        guard let (data, _) = try? await request("GET", "/stickers/packs"),
              let m = try? JSONDecoder().decode(StickerPacksDto.self, from: data) else { return nil }
        return m.packs
    }

    /// `GET /stickers/{pack}/{id}` — one sticker's bytes (the relay caches them; MIT / CC BY).
    func stickerData(path: String) async -> Data? {
        guard let (data, http) = try? await request("GET", path), http.statusCode == 200, !data.isEmpty else { return nil }
        return data
    }

    /// The relay's GIF pass-through for a provider media URL (`GET /gif/media?u=`); the caller
    /// adds the device bearer. `nil` when the URL cannot be encoded.
    func gifMediaURL(_ remote: String) -> URL? {
        guard let enc = remote.addingPercentEncoding(withAllowedCharacters: .alphanumerics) else { return nil }
        return URL(string: "\(endpoint.httpUrl)/gif/media?u=\(enc)")
    }

    /// `GET /link-icon?domain=` — the site's 64×64 PNG for the bubbles' named links, `nil` when the
    /// site has none (204) or the call failed (nobody asked; the bubble keeps its placeholder).
    func linkIcon(host: String) async -> Data? {
        guard let q = host.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) else { return nil }
        guard let (data, http) = try? await request("GET", "/link-icon?domain=\(q)") else { return nil }
        return http.statusCode == 200 && !data.isEmpty ? data : nil
    }

    func json(_ method: String, _ path: String, body: Any? = nil, auth: Bool = true) async throws -> Any {
        let (data, _) = try await request(method, path, body: body, auth: auth)
        if data.isEmpty { return NSNull() }
        return (try? JSONSerialization.jsonObject(with: data)) ?? NSNull()
    }

    /// A relay reply decoded as its `crates/arbos-core/src/api.rs` Dto.
    func decode<T: Decodable>(_ type: T.Type, _ method: String, _ path: String, body: Any? = nil) async throws -> T {
        let (data, _) = try await request(method, path, body: body)
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            NSLog("\(T.self): \(error)")
            throw RelayError(status: 0, message: "The relay returned an unreadable \(T.self)")
        }
    }

    // MARK: Pairing (D35)

    func pair(code: String, name: String) async throws -> DeviceCredentials {
        let (data, _) = try await request("POST", "/pair", body: ["code": code.trimmingCharacters(in: .whitespaces), "name": name], auth: false)
        let dto: PairResponseDto
        do {
            dto = try JSONDecoder().decode(PairResponseDto.self, from: data)
        } catch {
            NSLog("PairResponseDto: \(error)")
            throw RelayError(status: 0, message: "The relay returned no token")
        }
        guard !dto.device_token.isEmpty else {
            throw RelayError(status: 0, message: "The relay returned no token")
        }
        return DeviceCredentials(
            relayUrl: endpoint.httpUrl,
            deviceId: dto.device_id.isEmpty ? "device" : dto.device_id,
            deviceToken: dto.device_token
        )
    }

    // MARK: Registry

    /// `GET /projects` then `GET /projects/{id}` for chats, models and spend.
    func registry() async throws -> RegistryResult {
        guard let list = try await json("GET", "/projects") as? [Any] else {
            return RegistryResult(projects: [], skippedIds: [], incomplete: false)
        }
        var out: [RegistryProject] = []
        var skippedIds: Set<String> = []
        var incomplete = false
        let decoder = JSONDecoder()
        for item in list {
            guard JSONSerialization.isValidJSONObject(item),
                  let itemData = try? JSONSerialization.data(withJSONObject: item) else {
                incomplete = true
                continue
            }
            let id: String
            // The per-user archive flag rides on the list only; the detail is shared.
            var listArchivedAt: Int64? = nil
            if let dto = try? decoder.decode(ProjectDto.self, from: itemData) {
                id = dto.id
                listArchivedAt = dto.archived_at.flatMap { $0 }
            } else if let bare = try? decoder.decode(ProjectListId.self, from: itemData) {
                id = bare.id
            } else {
                NSLog("ProjectDto: bad list item")
                incomplete = true
                continue
            }
            // A project the list shows but this user may not open (403) is an
            // access answer, not a parse skip — leave it out of skippedIds.
            let detailRaw: Any
            do {
                detailRaw = try await json("GET", "/projects/\(id)")
            } catch let e as RelayError where e.status == 403 || e.status == 404 {
                continue
            }
            guard JSONSerialization.isValidJSONObject(detailRaw),
                  let detailData = try? JSONSerialization.data(withJSONObject: detailRaw) else {
                skippedIds.insert(id)
                continue
            }
            let detail: GetProjectResponseDto
            do {
                detail = try decoder.decode(GetProjectResponseDto.self, from: detailData)
            } catch {
                NSLog("GetProjectResponseDto: \(error)")
                skippedIds.insert(id)
                continue
            }
            var project = Arbos.Project(dto: detail.project)
            project.archivedAt = listArchivedAt
            project.imageURL = Arbos.Project.pictureURL(httpUrl: endpoint.httpUrl, id: id, imageKey: project.imageKey, version: project.avatarVersion)
            // Prefer the detail's live box word when present (same as the old parse).
            project.state = Arbos.BoxState.from(state: detail.project.state, box: detail.box)
            project.stateSince = detail.box_since
            // `models` is free-form JSON (skipped on the Dto); read talking model id + effort only.
            if let detailMap = detailRaw as? [String: Any] {
                let p = (detailMap["project"] as? [String: Any]) ?? [:]
                let models = p["models"] as? [String: Any] ?? [:]
                let talking = models["talking_model"] as? [String: Any] ?? [:]
                if let modelId = talking["id"] as? String, !modelId.isEmpty {
                    project.model = modelId
                }
                project.effort = Effort.from(talking["effort"] as? String)
            }
            let chats = detail.chats.map { Arbos.Chat(dto: $0) }
            out.append(RegistryProject(project: project, chats: chats))
        }
        return RegistryResult(projects: out, skippedIds: skippedIds, incomplete: incomplete)
    }

    static func parseChat(_ raw: Any) -> Arbos.Chat? {
        guard JSONSerialization.isValidJSONObject(raw),
              let data = try? JSONSerialization.data(withJSONObject: raw) else { return nil }
        do {
            return Arbos.Chat(dto: try JSONDecoder().decode(ChatDto.self, from: data))
        } catch {
            NSLog("ChatDto: \(error)")
            return nil
        }
    }

    func createChat(projectId: Id) async throws -> Arbos.Chat {
        let v = try await json("POST", "/projects/\(projectId)/chats", body: ["title": "New chat"])
        guard JSONSerialization.isValidJSONObject(v),
              let data = try? JSONSerialization.data(withJSONObject: v) else {
            throw RelayError(status: 0, message: "The relay returned no chat")
        }
        do {
            let resp = try JSONDecoder().decode(CreateChatResponseDto.self, from: data)
            return Arbos.Chat(dto: resp.chat)
        } catch {
            NSLog("CreateChatResponseDto: \(error)")
            throw RelayError(status: 0, message: "The relay returned no chat")
        }
    }


    func createProject(
        name: String,
        avatarColorIndex: Int? = nil,
        leaf: LeafPick? = nil,
        color: String? = nil,
        description: String? = nil,
        instructions: String? = nil,
        sourceProjectId: Id? = nil
    ) async throws -> Id? {
        var body: [String: Any] = ["name": name]
        if let avatarColorIndex { body["avatar_color_index"] = avatarColorIndex }
        // D92: the Create sheet's leaf + colour (forks copy the source's).
        if let leaf { body["avatar_leaf"] = Int(leaf.leaf); body["avatar_leaf_color"] = Int(leaf.color) }
        if let color { body["color"] = color }
        if let description, !description.isEmpty { body["description"] = String(description.prefix(500)) }
        if let instructions, !instructions.isEmpty { body["instructions"] = instructions }
        // Fork: relay copies the source agent's workspace (files + memory) into the new one.
        if let sourceProjectId { body["source_project_id"] = sourceProjectId }
        let v = try await json("POST", "/projects", body: body)
        return (v as? [String: Any])?["id"] as? String
    }

    // MARK: Outside agents (#2142) — an agent that runs elsewhere, hosted by the relay

    /// `POST /outside-agents {name, transport}` → the new agent's key, shown once.
    func connectAgent(name: String, transport: String) async throws -> ConnectReveal {
        let v = try await json("POST", "/outside-agents", body: ["name": name, "transport": transport])
        guard let reveal = ConnectReveal(json: v, rotated: false) else {
            throw RelayError(status: 500, message: "The relay answered without a key.")
        }
        return reveal
    }

    /// `GET /outside-agents/{id}` → its connection; `nil` for an agent that is not outside.
    func outsideAgent(_ projectId: Id) async throws -> OutsideAgentInfo? {
        do {
            let v = try await json("GET", "/outside-agents/\(projectId)")
            let d = v as? [String: Any]
            return OutsideAgentInfo(json: d?["agent"], log: d?["log"])
        } catch let e as RelayError where e.status == 404 {
            return nil
        }
    }

    /// `GET /outside-agents/{id}/log?limit=500` (#2224): the connected agent's session log,
    /// oldest first; an older relay (404) reads as no log.
    func outsideAgentLog(_ projectId: Id) async throws -> [OutsideLogEntry] {
        do {
            let v = try await json("GET", "/outside-agents/\(projectId)/log?limit=500")
            let entries = (v as? [String: Any])?["entries"] as? [Any] ?? []
            return entries.compactMap { OutsideLogEntry(json: $0) }
        } catch let e as RelayError where e.status == 404 {
            return []
        }
    }

    /// `PATCH /outside-agents/{id} {scopes: {name: on}}` — one switch of the Can card → the
    /// agent with its effective set.
    func setOutsideScope(_ projectId: Id, _ scope: String, on: Bool) async throws -> OutsideAgentInfo? {
        let v = try await json("PATCH", "/outside-agents/\(projectId)", body: ["scopes": [scope: on]])
        return OutsideAgentInfo(json: (v as? [String: Any])?["agent"])
    }

    /// `POST /outside-agents/{id}/rotate` → a fresh key; the old one stops at once.
    func rotateOutsideKey(_ projectId: Id) async throws -> ConnectReveal {
        let v = try await json("POST", "/outside-agents/\(projectId)/rotate", body: [String: Any]())
        guard let reveal = ConnectReveal(json: v, rotated: true) else {
            throw RelayError(status: 500, message: "The relay answered without a key.")
        }
        return reveal
    }

    /// `DELETE /outside-agents/{id}`: revoke the key; the agent and its messages stay.
    func revokeOutsideAgent(_ projectId: Id) async throws {
        _ = try await request("DELETE", "/outside-agents/\(projectId)")
    }

    /// `PATCH /outside-agents/{id} {wake_url, wake_auth}` ([feedback outside-agents-wake-ios-2026-10-28]):
    /// `nil` leaves a field alone, an empty string clears it. → the agent's connection.
    func setOutsideWake(_ projectId: Id, url: String?, auth: String?) async throws -> OutsideAgentInfo? {
        var body: [String: Any] = [:]
        if let url { body["wake_url"] = url.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? NSNull() : url }
        if let auth { body["wake_auth"] = auth.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? NSNull() : auth }
        let v = try await json("PATCH", "/outside-agents/\(projectId)", body: body)
        return OutsideAgentInfo(json: (v as? [String: Any])?["agent"])
    }

    /// `POST /outside-agents/{id}/wake/test` → `(ok, "Delivered · HTTP 200" | "Failed · <reason>")`.
    func testOutsideWake(_ projectId: Id) async throws -> (ok: Bool, text: String) {
        let v = try await json("POST", "/outside-agents/\(projectId)/wake/test", body: [String: Any]())
        let d = v as? [String: Any] ?? [:]
        let ok = d["ok"] as? Bool ?? false
        if ok {
            if let s = d["status"] as? NSNumber { return (true, "Delivered · HTTP \(s.intValue)") }
            return (true, "Delivered")
        }
        if let e = d["error"] as? String, !e.isEmpty { return (false, "Failed · \(e)") }
        return (false, "Failed")
    }

    /// Soft-delete an owned agent (`DELETE /projects/{id}`).
    func deleteProject(_ projectId: Id) async throws {
        _ = try await request("DELETE", "/projects/\(projectId)")
    }

    /// `PUT /projects/{id}/archive {archived}` — this user's sidebar archive, synced across devices.
    func setProjectArchived(_ projectId: Id, archived: Bool) async throws {
        _ = try await request("PUT", "/projects/\(projectId)/archive", body: ["archived": archived])
    }

    /// `PUT /servers/{id}/archive {archived}` — same flag for a server row.
    func setServerArchived(_ serverId: String, archived: Bool) async throws {
        _ = try await request("PUT", "/servers/\(serverId)/archive", body: ["archived": archived])
    }

    /// `GET /me/pins` → my sidebar / Home pins, top first, and the relay's cap
    /// ([feedback sidebar-pinning-ios-2026-10-28]).
    func pins() async throws -> PinsDto {
        try await decode(PinsDto.self, "GET", "/me/pins")
    }

    /// `PUT /me/pins {pins}` replaces the whole list; the answer is the list the relay kept.
    /// Over the cap → `409 pin_cap`.
    func setPins(_ pins: [String]) async throws -> PinsDto {
        try await decode(PinsDto.self, "PUT", "/me/pins", body: ["pins": pins])
    }

    func setModel(projectId: Id, model: String, effort: Effort) async throws {
        let body: [String: Any] = ["models": ["talking_model": ["id": model, "effort": effort.rawValue]]]
        _ = try await request("PATCH", "/projects/\(projectId)", body: body)
    }

    func wake(projectId: Id) async throws {
        _ = try await json("POST", "/projects/\(projectId)/wake", body: [String: Any]())
    }

    /// The tasks chip's poll (duplex §2.4): `GET /projects/{id}` → `activity.jobs[]`. A relay
    /// without the field or a `null` activity reads as no open jobs.
    func projectJobs(projectId: Id) async throws -> [Job] {
        let raw = try await json("GET", "/projects/\(projectId)")
        guard let detail = raw as? [String: Any] else { return [] }
        return Job.list(fromActivity: detail["activity"])
    }

    /// The job tab's document: `GET /projects/{id}/jobs/{jobId}[?after=<seq>]`
    /// (`internal/duplex-rollout-client-contract.md`). nil when the relay has no such endpoint
    /// (404) — the tab says so; other errors throw.
    func jobRollout(projectId: Id, jobId: String, after: Int64?) async throws -> JobRollout? {
        let path = after.map { "/projects/\(projectId)/jobs/\(jobId)?after=\($0)" } ?? "/projects/\(projectId)/jobs/\(jobId)"
        do {
            let raw = try await json("GET", path)
            return JobRollout(raw)
        } catch let e as RelayError where e.status == 404 {
            return nil
        }
    }

    // MARK: Machine (D70: one box per user, projects are workspaces on it)

    /// `GET /machine` → the caller's one box and its workspaces.
    func machine() async throws -> Machine {
        let v = try await json("GET", "/machine")
        guard JSONSerialization.isValidJSONObject(v),
              let data = try? JSONSerialization.data(withJSONObject: v) else {
            throw RelayError(status: 0, message: "Bad /machine response")
        }
        let m: MachineStatusDto
        do {
            m = try JSONDecoder().decode(MachineStatusDto.self, from: data)
        } catch {
            NSLog("MachineStatusDto: \(error)")
            throw RelayError(status: 0, message: "Bad /machine response")
        }
        return Machine(dto: m)
    }

    func startMachine() async throws {
        _ = try await json("POST", "/machine/start", body: [String: Any]())
    }

    func stopMachine() async throws {
        _ = try await json("POST", "/machine/stop", body: [String: Any]())
    }

    /// Always-on boxes (D73) are restarted, never stopped. Uses
    /// `POST /machine/restart` when the relay has it and falls back to
    /// stop → start on relays that predate the route.
    func restartMachine() async throws {
        do {
            _ = try await json("POST", "/machine/restart", body: [String: Any]())
        } catch let error as RelayError where error.status == 404 || error.status == 405 {
            try await stopMachine()
            try await startMachine()
        }
    }

    /// Model ids the relay proxy can serve, filtered like the Mac client,
    /// then trimmed to the newest major of each family and sorted newest-first.
    func models() async throws -> [String] {
        let (data, _) = try await request("GET", "/proxy/v1/models")
        let list = (try? JSONDecoder().decode(ModelListDto.self, from: data))?.data ?? []
        let allowed = ["anthropic/", "openai/", "google/", "x-ai/"]
        var ids = list.map(\.id)
            .filter { id in !id.contains(":") && allowed.contains { id.hasPrefix($0) } }
        var seen = Set<String>()
        ids = ids.filter { seen.insert($0).inserted }
        ids = Self.currentModels(ids)
        ids.sort(by: Self.newerModel)
        return ids
    }

    /// Drop older majors in the same provider/stem family
    /// (`google/gemini-2.5-*` goes away when any `google/gemini-3*` is listed).
    private static func currentModels(_ ids: [String]) -> [String] {
        var maxMajor: [String: Int] = [:]
        for id in ids {
            let key = modelFamily(id)
            if let major = modelVersion(id).first {
                maxMajor[key] = max(maxMajor[key] ?? major, major)
            }
        }
        return ids.filter { id in
            guard let major = modelVersion(id).first, let keep = maxMajor[modelFamily(id)] else {
                return true
            }
            return major == keep
        }
    }

    /// `google/gemini-3.1-flash` → `google/gemini`.
    private static func modelFamily(_ id: String) -> String {
        let parts = id.split(separator: "/", maxSplits: 1).map(String.init)
        let provider = parts.first ?? ""
        let leaf = parts.count > 1 ? parts[1] : id
        let stem = leaf.prefix(while: \.isLetter)
        return "\(provider)/\(stem)"
    }

    /// Number groups in the leaf id (`gemini-3.1-flash` → `[3, 1]`). Trailing
    /// date stamps (`…-20241022`) are dropped so they do not look like versions.
    private static func modelVersion(_ id: String) -> [Int] {
        let leaf = id.split(separator: "/").last.map(String.init) ?? id
        var nums: [Int] = []
        var current = ""
        for ch in leaf {
            if ch.isNumber {
                current.append(ch)
            } else if !current.isEmpty {
                if let n = Int(current) { nums.append(n) }
                current = ""
            }
        }
        if !current.isEmpty, let n = Int(current) { nums.append(n) }
        if let last = nums.last, last >= 10_000_000 { nums.removeLast() }
        return nums
    }

    /// Newer first: higher version within a family, else reverse id order.
    private static func newerModel(_ a: String, _ b: String) -> Bool {
        let fa = modelFamily(a), fb = modelFamily(b)
        if fa == fb {
            let va = modelVersion(a), vb = modelVersion(b)
            let n = min(va.count, vb.count)
            for i in 0..<n where va[i] != vb[i] { return va[i] > vb[i] }
            if va.count != vb.count { return va.count > vb.count }
        }
        return a > b
    }

    /// Upload an attachment to the relay blob mirror; returns the key.
    func putBlob(projectId: Id, key: String, mime: String, data: Data) async throws -> String {
        guard let url = URL(string: "\(endpoint.httpUrl)/blob/\(projectId)/\(key)") else {
            throw RelayError(status: 0, message: "Bad relay URL")
        }
        var req = URLRequest(url: url)
        req.httpMethod = "PUT"
        req.setValue("Bearer \(token())", forHTTPHeaderField: "Authorization")
        req.setValue(mime, forHTTPHeaderField: "Content-Type")
        req.httpBody = data
        let (body, response) = try await session.data(for: req)
        guard let http = response as? HTTPURLResponse, http.statusCode < 400 else {
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            throw RelayError.from(status: status, body: body)
        }
        return key
    }

    /// `PUT /me/avatar` (raw PNG or JPEG master) → `MeUserResponseDto`; updates the signed-in
    /// user's picture. The relay checks that the type and the bytes agree.
    func putMeAvatar(jpeg: Data) async throws -> String {
        guard let url = URL(string: "\(endpoint.httpUrl)/me/avatar") else {
            throw RelayError(status: 0, message: "Bad relay URL")
        }
        var req = URLRequest(url: url)
        req.httpMethod = "PUT"
        req.setValue("Bearer \(token())", forHTTPHeaderField: "Authorization")
        req.setValue(PictureMaster.mime(jpeg), forHTTPHeaderField: "Content-Type")
        req.httpBody = jpeg
        let (body, response) = try await session.data(for: req)
        guard let http = response as? HTTPURLResponse, http.statusCode < 400 else {
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            throw RelayError.from(status: status, body: body)
        }
        guard let dto = try? JSONDecoder().decode(MeUserResponseDto.self, from: body) else {
            throw RelayError(status: 0, message: "The relay returned no avatar URL")
        }
        if let url = dto.avatar_url.flatMap({ $0 }), !url.isEmpty { return url }
        if let url = dto.user.avatar_url.flatMap({ $0 }), !url.isEmpty { return url }
        throw RelayError(status: 0, message: "The relay returned no avatar URL")
    }

    /// `POST /feedback` (JSON, device token): `{note ≤4000, image: base64 PNG/JPEG ≤15 MB,
    /// mime, app_version, platform: "ios", os, project_id?, width, height}` → 201 `{id, created_at}`.
    /// Feeds the same automatic fix loop as the Mac sheet.
    func sendFeedback(note: String, image: Data?, mime: String?, width: Int, height: Int,
                      appVersion: String, os: String, projectId: Id?) async throws -> String? {
        var body: [String: Any] = [
            "note": String(note.prefix(4000)), "app_version": appVersion, "platform": "ios", "os": os,
            "width": width, "height": height,
        ]
        if let image, let mime {
            body["image"] = image.base64EncodedString()
            body["mime"] = mime
        }
        if let projectId, !projectId.isEmpty { body["project_id"] = projectId }
        return try await decode(FeedbackSubmitReplyDto.self, "POST", "/feedback", body: body).id
    }

    /// `POST /perf/report` (JSON, device token): one MetricKit payload in its envelope
    /// (`PerfDiagnostics.envelope`) → `{id}`. Anonymous on the relay side.
    func sendPerfReport(_ body: [String: Any]) async throws {
        _ = try await json("POST", "/perf/report", body: body)
    }

    /// `POST /feedback/capture/{id}` (device token): the answer to a `feedback_capture` push from
    /// the user's agent (`submit_feedback`). `{image: base64 JPEG/PNG, mime, width, height,
    /// platform: "ios", app_version, os, device, screen}`; or `{skipped: "<why>"}` with no image.
    func sendFeedbackCapture(requestId: String, image: Data?, mime: String, width: Int, height: Int,
                             appVersion: String, os: String, device: String, screen: String, skipped: String?) async throws {
        var body: [String: Any] = [
            "platform": "ios", "app_version": appVersion, "os": os, "device": device, "screen": screen,
        ]
        if let image {
            body["image"] = image.base64EncodedString()
            body["mime"] = mime
            body["width"] = width
            body["height"] = height
        }
        if let skipped { body["skipped"] = skipped }
        let safe = requestId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? requestId
        _ = try await json("POST", "/feedback/capture/\(safe)", body: body)
    }

    /// `POST /proxy/v1/audio/transcriptions` (multipart `file` + `model`) → the memo's text.
    /// The relay meters it on `projectId` (`X-Arbos-Project`).
    func transcribe(projectId: Id?, data: Data, mime: String) async throws -> String {
        guard let url = URL(string: "\(endpoint.httpUrl)/proxy/v1/audio/transcriptions") else {
            throw RelayError(status: 0, message: "Bad relay URL")
        }
        let boundary = "arbos-\(UUID().uuidString)"
        var body = Data()
        // Match Mac: mini-transcribe is much faster than whisper-1 for short voice memos.
        body.append(Data("--\(boundary)\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngpt-4o-mini-transcribe\r\n".utf8))
        body.append(Data("--\(boundary)\r\nContent-Disposition: form-data; name=\"file\"; filename=\"memo.m4a\"\r\nContent-Type: \(mime)\r\n\r\n".utf8))
        body.append(data)
        body.append(Data("\r\n--\(boundary)--\r\n".utf8))
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.timeoutInterval = 60
        req.setValue("Bearer \(token())", forHTTPHeaderField: "Authorization")
        req.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        if let projectId { req.setValue(projectId, forHTTPHeaderField: "X-Arbos-Project") }
        req.httpBody = body
        let (reply, response) = try await session.data(for: req)
        guard let http = response as? HTTPURLResponse, http.statusCode < 400 else {
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            throw RelayError(status: status, message: String(data: reply, encoding: .utf8) ?? "")
        }
        guard let dto = try? JSONDecoder().decode(TranscriptionDto.self, from: reply) else {
            throw RelayError(status: http.statusCode, message: "No transcript in the reply")
        }
        return dto.text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    // MARK: Calls v1

    /// `POST /chats/{id}/calls` (or the projects form) `{kind, ring?}` → 201 new call / 200 join;
    /// both carry a fresh LiveKit token. `ring` is an optional list of member ids to notify
    /// (bare user ids and agent project ids); omit or empty = the whole group. Returns the
    /// call and whether we started it.
    func startCall(chatId: String, projectId: String?, kind: CallKind, ring: [String]? = nil) async throws -> (Call, started: Bool) {
        let path = (projectId == nil || projectId == peopleSlot) ? "/chats/\(chatId)/calls" : "/projects/\(projectId!)/chats/\(chatId)/calls"
        var body: [String: Any] = ["kind": kind.rawValue]
        if let ring, !ring.isEmpty { body["ring"] = ring }
        let (data, http) = try await request("POST", path, body: body, headers: ["X-Arbos-Client": RelayAPI.clientHeader])
        let resp: StartCallResponseDto
        do {
            resp = try JSONDecoder().decode(StartCallResponseDto.self, from: data)
        } catch {
            NSLog("StartCallResponseDto: \(error)")
            throw RelayError(status: http.statusCode, message: "Bad call response")
        }
        let reachable = (resp.reachable ?? []).map(CallReachability.init(dto:))
        let call = Call(dto: resp.call, reachable: reachable)
        // 201 {call, joined: false} = we started it; 200 {call, joined: true} = we were added.
        return (call, !resp.joined)
    }

    func joinCall(_ callId: String) async throws -> Call {
        let data = try await request("POST", "/calls/\(callId)/join", body: [String: Any]()).0
        do {
            let env = try JSONDecoder().decode(CallEnvelopeDto.self, from: data)
            return Call(dto: env.call)
        } catch {
            NSLog("CallEnvelopeDto: \(error)")
            throw RelayError(status: 0, message: "Bad call response")
        }
    }

    /// `POST /calls/{id}/leave` → `{ended, duration_s}`; the client told `ended: true` writes the end line.
    func leaveCall(_ callId: String) async throws -> (ended: Bool, durationS: Int) {
        let data = try await request("POST", "/calls/\(callId)/leave", body: [String: Any]()).0
        let resp = (try? JSONDecoder().decode(LeaveCallResponseDto.self, from: data))
            ?? LeaveCallResponseDto(ended: false, duration_s: 0, already_ended: nil)
        return (resp.ended, Int(resp.duration_s))
    }

    /// `POST /calls/{id}/speakers {user_id}` — host invites someone to speak on a hosted group call.
    func inviteSpeaker(callId: String, userId: String) async throws -> Call {
        let data = try await request("POST", "/calls/\(callId)/speakers", body: ["user_id": userId]).0
        do {
            let env = try JSONDecoder().decode(CallEnvelopeDto.self, from: data)
            return Call(dto: env.call)
        } catch {
            NSLog("CallEnvelopeDto inviteSpeaker: \(error)")
            throw RelayError(status: 0, message: "Bad call response")
        }
    }

    /// `DELETE /calls/{id}/speakers/{user_id}` — host revokes speak permission.
    func revokeSpeaker(callId: String, userId: String) async throws -> Call {
        let data = try await request("DELETE", "/calls/\(callId)/speakers/\(userId)").0
        do {
            let env = try JSONDecoder().decode(CallEnvelopeDto.self, from: data)
            return Call(dto: env.call)
        } catch {
            NSLog("CallEnvelopeDto revokeSpeaker: \(error)")
            throw RelayError(status: 0, message: "Bad call response")
        }
    }

    /// `GET /calls/{id}` → the call, also after it ended (no token then).
    func call(_ callId: String) async throws -> Call {
        let data = try await request("GET", "/calls/\(callId)").0
        do {
            let env = try JSONDecoder().decode(CallEnvelopeDto.self, from: data)
            return Call(dto: env.call)
        } catch {
            NSLog("CallEnvelopeDto: \(error)")
            throw RelayError(status: 0, message: "Bad call response")
        }
    }

    /// `GET /chats/{id}/call` (or the projects form) → the active call, nil on 404.
    func activeCall(chatId: String, projectId: String?) async throws -> Call? {
        let path = (projectId == nil || projectId == peopleSlot) ? "/chats/\(chatId)/call" : "/projects/\(projectId!)/chats/\(chatId)/call"
        do {
            let data = try await request("GET", path).0
            let env = try JSONDecoder().decode(CallEnvelopeDto.self, from: data)
            return Call(dto: env.call)
        } catch let e as RelayError where e.status == 404 {
            return nil
        } catch {
            NSLog("CallEnvelopeDto: \(error)")
            return nil
        }
    }

    /// `POST /devices/{id}/voip {token, environment, bundle_id}` → `{push}` (false until the APNs key exists).
    func registerVoip(deviceId: Id, token: String, environment: String, bundleId: String) async throws -> Bool {
        let data = try await request("POST", "/devices/\(deviceId)/voip", body: ["token": token, "environment": environment, "bundle_id": bundleId]).0
        let resp = try? JSONDecoder().decode(VoipRegisterResponseDto.self, from: data)
        return resp?.push ?? true
    }

    func unregisterVoip(deviceId: Id) async throws {
        _ = try await request("DELETE", "/devices/\(deviceId)/voip")
    }

    // MARK: Push notifications

    /// `POST /devices/{id}/push {platform, token, environment, bundle_id, app_version}`
    /// registers this device's APNs token (idempotent; re-sent on every launch).
    /// Returns the relay's `push` flag: false until it has an APNs key.
    func registerPush(deviceId: Id, token: String, environment: String, bundleId: String, appVersion: String) async throws -> Bool {
        let data = try await request("POST", "/devices/\(deviceId)/push", body: [
            "platform": "ios", "token": token, "environment": environment, "bundle_id": bundleId, "app_version": appVersion,
        ]).0
        let resp = try? JSONDecoder().decode(PushRegisterResponseDto.self, from: data)
        return resp?.push ?? true
    }

    /// `DELETE /devices/{id}/push` on sign-out so a stale token is not pushed to.
    func unregisterPush(deviceId: Id) async throws {
        _ = try await request("DELETE", "/devices/\(deviceId)/push")
    }

    /// `GET /me/unread` → total + per-chat counts, and per-chat unread mentions of me.
    /// nil when the relay has no route yet. `mentions` holds only the chats with one, so a
    /// relay that does not count them yet leaves it empty rather than zero-filled — the DTO
    /// field is optional (`#[serde(default)]` on the Rust side), and absent means none.
    func unread() async throws -> (total: Int, chats: [String: Int], mentions: [String: Int])? {
        do {
            let v = try await json("GET", "/me/unread")
            guard JSONSerialization.isValidJSONObject(v),
                  let data = try? JSONSerialization.data(withJSONObject: v),
                  let dto = try? JSONDecoder().decode(UnreadDto.self, from: data) else { return nil }
            var chats: [String: Int] = [:]
            var mentions: [String: Int] = [:]
            for c in dto.chats {
                chats[c.chat_id] = Int(c.unread)
                let named = Int(c.mentions ?? 0)
                if named > 0 { mentions[c.chat_id] = named }
            }
            return (Int(dto.total), chats, mentions)
        } catch let e as RelayError where e.status == 404 || e.status == 405 {
            return nil
        }
    }

    // MARK: People chats (D72)

    /// `GET /users/{id}` → one person with presence (§9); nil on 404.
    func user(_ id: String) async throws -> Person? {
        do { return Person.parse(try await json("GET", "/users/\(id)")) }
        catch let e as RelayError where e.status == 404 { return nil }
    }

    /// `GET /users?offset&limit` → the directory (members and invited people
    /// except me), alphabetical, paged by `next_offset`.
    func users() async throws -> [Person] {
        var out: [Person] = []
        var offset = 0
        while true {
            let v = try await json("GET", "/users?offset=\(offset)&limit=500")
            guard JSONSerialization.isValidJSONObject(v),
                  let data = try? JSONSerialization.data(withJSONObject: v),
                  let page = try? JSONDecoder().decode(UsersPageDto.self, from: data) else { break }
            out += page.users.map(Person.init(dto:))
            guard let next = page.next_offset.map(Int.init), next > offset else { break }
            offset = next
        }
        return out
    }

    func searchUsers(_ q: String) async throws -> [Person] {
        let query = q.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "@"))
        guard !query.isEmpty, let enc = query.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) else { return [] }
        let v = try await json("GET", "/users/search?q=\(enc)")
        guard JSONSerialization.isValidJSONObject(v),
              let data = try? JSONSerialization.data(withJSONObject: v),
              let cards = try? JSONDecoder().decode([UserCardDto].self, from: data) else { return [] }
        return cards.map(Person.init(dto:))
    }

    func openDM(handle: String) async throws -> PeopleChat {
        let v = try await json("POST", "/chats/dm", body: ["handle": handle.trimmingCharacters(in: CharacterSet(charactersIn: "@ "))])
        guard let chat = PeopleChat.parse(v) else { throw RelayError(status: 0, message: "The relay returned no chat") }
        return chat
    }

    func createGroup(handles: [String], agentIds: [Id] = [], title: String? = nil, leaf: LeafPick? = nil) async throws -> PeopleChat {
        var body: [String: Any] = ["kind": "group", "members": handles, "agent_ids": agentIds]
        if let title, !title.isEmpty { body["title"] = title }
        if let leaf { body["avatar_leaf"] = Int(leaf.leaf); body["avatar_leaf_color"] = Int(leaf.color) }
        let v = try await json("POST", "/chats", body: body)
        guard let chat = PeopleChat.parse(v) else { throw RelayError(status: 0, message: "The relay returned no chat") }
        return chat
    }

    // MARK: Servers (D88)

    /// `GET /servers` → every server I belong to (channels, my role, invite for owner/admin).
    func servers() async throws -> [Server] {
        ((try await json("GET", "/servers")) as? [Any] ?? []).compactMap(Server.parse)
    }

    /// `GET /servers/{id}` → the server with its members.
    func server(_ id: String) async throws -> Server {
        guard let s = Server.parse(try await json("GET", "/servers/\(id)")) else { throw RelayError(status: 0, message: "Bad server") }
        return s
    }

    /// `POST /servers {name}` → 201 the server (with `#general` and members).
    func createServer(name: String, leaf: LeafPick? = nil) async throws -> Server {
        var body: [String: Any] = ["name": name.trimmingCharacters(in: .whitespacesAndNewlines)]
        if let leaf { body["avatar_leaf"] = Int(leaf.leaf); body["avatar_leaf_color"] = Int(leaf.color) }
        let v = try await json("POST", "/servers", body: body)
        guard let s = Server.parse(v) else { throw RelayError(status: 0, message: "The relay returned no server") }
        return s
    }

    /// `PATCH /servers/{id} {name?, image_key?, open?}` (owner/admin).
    func patchServer(_ id: String, name: String? = nil, imageKey: String? = nil, open: Bool? = nil) async throws {
        var body: [String: Any] = [:]
        if let name { body["name"] = name }
        if let imageKey { body["image_key"] = imageKey }
        if let open { body["open"] = open }
        guard !body.isEmpty else { return }
        _ = try await request("PATCH", "/servers/\(id)", body: body)
    }

    /// D92 amendment: a server's leaf picture — `PATCH /servers/{id} {avatar_leaf,
    /// avatar_leaf_color, image_key: null}` (nil = back to initials; the picture goes with it).
    func setServerLeaf(_ id: String, pick: LeafPick?) async throws {
        let body: [String: Any] = [
            "avatar_leaf": pick.map { Int($0.leaf) as Any } ?? NSNull(),
            "avatar_leaf_color": pick.map { Int($0.color) as Any } ?? NSNull(),
            "image_key": NSNull(),
        ]
        _ = try await request("PATCH", "/servers/\(id)", body: body)
    }

    /// `GET /servers/search?q=` → open servers by name (join without an invite).
    func searchOpenServers(_ q: String) async throws -> [Server] {
        let enc = q.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? q
        return ((try await json("GET", "/servers/search?q=\(enc)")) as? [Any] ?? []).compactMap(Server.parse)
    }

    /// `DELETE /servers/{id}` (owner).
    func deleteServer(_ id: String) async throws {
        _ = try await request("DELETE", "/servers/\(id)")
    }

    /// `POST /servers/{id}/channels {name, topic?, restricted?}` → 201 the channel.
    func createChannel(serverId: String, name: String, topic: String? = nil, restricted: Bool = false) async throws -> ServerChannel {
        var body: [String: Any] = ["name": name, "restricted": restricted]
        if let topic, !topic.isEmpty { body["topic"] = topic }
        let v = try await json("POST", "/servers/\(serverId)/channels", body: body)
        guard let c = ServerChannel.parse(v) else { throw RelayError(status: 0, message: "The relay returned no channel") }
        return c
    }

    /// `PATCH /servers/{id}/channels/{cid} {name?, topic?, restricted?}`.
    func patchChannel(serverId: String, channelId: String, name: String? = nil, topic: String? = nil, restricted: Bool? = nil) async throws {
        var body: [String: Any] = [:]
        if let name { body["name"] = name }
        if let topic { body["topic"] = topic }
        if let restricted { body["restricted"] = restricted }
        guard !body.isEmpty else { return }
        _ = try await request("PATCH", "/servers/\(serverId)/channels/\(channelId)", body: body)
    }

    /// `DELETE /servers/{id}/channels/{cid}` (`#general` answers 409).
    func deleteChannel(serverId: String, channelId: String) async throws {
        _ = try await request("DELETE", "/servers/\(serverId)/channels/\(channelId)")
    }

    /// `POST /servers/{id}/channels/{cid}/archive {archived}` — hide the channel from my list
    /// (per user; the room's `m.lowpriority` tag on the homeserver) or bring it back.
    func archiveChannel(serverId: String, channelId: String, archived: Bool) async throws {
        _ = try await request("POST", "/servers/\(serverId)/channels/\(channelId)/archive", body: ["archived": archived])
    }

    /// `POST /servers/{id}/members {user_id}` — owner/admin adds someone on Arbos by name.
    func addServerMember(serverId: String, userId: String) async throws {
        _ = try await request("POST", "/servers/\(serverId)/members", body: ["user_id": userId])
    }

    /// `POST /servers/{id}/invites {expires, max_uses}` (owner) → the new link.
    func createServerInvite(_ id: String, expires: Bool, maxUses: Int?) async throws -> ServerInviteLink? {
        var body: [String: Any] = ["expires": expires]
        if let maxUses { body["max_uses"] = maxUses }
        return ServerInviteLink.parse(try await json("POST", "/servers/\(id)/invites", body: body))
    }

    /// `DELETE /servers/{id}/invites/{iid}` (owner) — the link stops working at once.
    func revokeServerInvite(_ id: String, inviteId: String) async throws {
        _ = try await request("DELETE", "/servers/\(id)/invites/\(inviteId)")
    }

    /// `POST /servers/{id}/invite/reset` (owner) → the new `{code, url}`.
    func resetServerInvite(_ id: String) async throws -> ServerInvite {
        guard let inv = ServerInvite.parse(try await json("POST", "/servers/\(id)/invite/reset", body: [String: Any]())) else {
            throw RelayError(status: 0, message: "The relay returned no invite link")
        }
        return inv
    }

    /// `POST /servers/join {code}` (the code or the whole link) → the server.
    func joinServer(code: String) async throws -> Server {
        let v = try await json("POST", "/servers/join", body: ["code": code.trimmingCharacters(in: .whitespacesAndNewlines)])
        guard let s = Server.parse(v) else { throw RelayError(status: 0, message: "The relay returned no server") }
        return s
    }

    /// `POST /servers/join {server_id}` — open server, no invite link needed.
    func joinOpenServer(id: String) async throws -> Server {
        let v = try await json("POST", "/servers/join", body: ["server_id": id])
        guard let s = Server.parse(v) else { throw RelayError(status: 0, message: "The relay returned no server") }
        return s
    }

    /// `PUT /servers/{id}/members/{uid} {role: admin|member}` (owner).
    func setServerMemberRole(serverId: String, userId: String, role: String) async throws {
        _ = try await request("PUT", "/servers/\(serverId)/members/\(userId)", body: ["role": role])
    }

    /// `DELETE /servers/{id}/members/{uid}[?ban=1]` (owner/admin).
    func removeServerMember(serverId: String, userId: String, ban: Bool) async throws {
        _ = try await request("DELETE", "/servers/\(serverId)/members/\(userId)\(ban ? "?ban=1" : "")")
    }

    /// `POST /servers/{id}/leave`.
    func leaveServer(_ id: String) async throws {
        _ = try await request("POST", "/servers/\(id)/leave", body: [String: Any]())
    }

    /// `GET /me/matrix` → this device's session for the shared Matrix client; nil when the
    /// relay has no servers (503 `servers_off`) or predates the route (404).
    func matrixSession() async throws -> MatrixCredentials? {
        do {
            return MatrixCredentials.parse(try await json("GET", "/me/matrix"))
        } catch let e as RelayError where e.status == 503 || e.status == 404 || e.type == "servers_off" {
            return nil
        }
    }

    func patchProject(_ projectId: Id, name: String?, color: String?, icon: String?, avatar: String? = nil, avatarColor: String? = nil, description: String? = nil) async throws {
        var body: [String: Any] = [:]
        if let description { body["description"] = String(description.prefix(500)) }
        if let name { body["name"] = name }
        if let color { body["color"] = color }
        if let icon { body["icon"] = icon }
        if let avatar { body["avatar"] = avatar }
        // "mono" travels as a slug: a null would make the relay re-pick its coloured default (D74).
        if let avatarColor { body["avatar_color"] = avatarColor }
        guard !body.isEmpty else { return }
        _ = try await request("PATCH", "/projects/\(projectId)", body: body)
    }

    /// D92 `PATCH /projects/{id} {"avatar_leaf": 0–8|null, "avatar_leaf_color": 0–9|null}` (null =
    /// automatic). A leaf pick is "not a photo": the picture goes with it (`image_key: null`).
    func setProjectAvatarLeaf(_ projectId: Id, leaf: Int?, color: Int?) async throws -> Int {
        let body: [String: Any] = [
            "avatar_leaf": leaf.map { $0 as Any } ?? NSNull(),
            "avatar_leaf_color": color.map { $0 as Any } ?? NSNull(),
            "image_key": NSNull(),
        ]
        return Self.avatarVersion(of: try await json("PATCH", "/projects/\(projectId)", body: body))
    }

    /// The relay's stamp of the avatar change a `PATCH /projects/{id}` answered with
    /// (`avatar_version`); 0 from an older relay.
    static func avatarVersion(of reply: Any) -> Int {
        let d = reply as? [String: Any]
        let v = d?["avatar_version"] ?? (d?["project"] as? [String: Any])?["avatar_version"]
        return (v as? NSNumber)?.intValue ?? 0
    }

    /// D92 amendment: `PATCH /projects/{id} {"image_key": key}` — a blob already in the project's
    /// slot (uploaded with `putBlob`, or the Generate tab's answer) becomes the agent's picture.
    func setProjectImageKey(_ projectId: Id, key: String) async throws -> Int {
        Self.avatarVersion(of: try await json("PATCH", "/projects/\(projectId)", body: ["image_key": key]))
    }

    /// D92 amendment: `POST /projects/{id}/avatar/generate {prompt}` → the stored picture's key
    /// (not yet set; "Set avatar" sends it through `setProjectImageKey`).
    func generateProjectAvatar(_ projectId: Id, prompt: String) async throws -> String {
        try await generateAvatar(path: "/projects/\(projectId)/avatar/generate", prompt: prompt)
    }

    /// The same for a people group (`POST /chats/{id}/avatar/generate`) and a server
    /// (`POST /servers/{id}/avatar/generate`); the key lands in their own blob slot.
    func generateGroupAvatar(_ chatId: String, prompt: String) async throws -> String {
        try await generateAvatar(path: "/chats/\(chatId)/avatar/generate", prompt: prompt)
    }

    func generateServerAvatar(_ serverId: String, prompt: String) async throws -> String {
        try await generateAvatar(path: "/servers/\(serverId)/avatar/generate", prompt: prompt)
    }

    /// `POST /me/avatar/generate {prompt}` (#1789): a picture in my own slot `users:{me}` — the
    /// own-profile picker's route (Jacob, 2026-10-25; "Set avatar" sends the key through
    /// `putMeAvatarKey`), and the Create sheets' (the entity does not exist yet; the bytes are
    /// copied into its slot on Create).
    func generateMyAvatar(prompt: String) async throws -> String {
        try await generateAvatar(path: "/me/avatar/generate", prompt: prompt)
    }

    /// `PUT /me/avatar {image_key}` (JSON): the generated preview becomes my profile picture;
    /// answers the new `avatar_url`.
    func putMeAvatarKey(_ key: String) async throws -> String {
        let v = try await json("PUT", "/me/avatar", body: ["image_key": key])
        guard JSONSerialization.isValidJSONObject(v),
              let data = try? JSONSerialization.data(withJSONObject: v),
              let dto = try? JSONDecoder().decode(MeUserResponseDto.self, from: data) else {
            throw RelayError(status: 0, message: "The relay returned no avatar URL")
        }
        if let url = dto.avatar_url.flatMap({ $0 }), !url.isEmpty { return url }
        if let url = dto.user.avatar_url.flatMap({ $0 }), !url.isEmpty { return url }
        throw RelayError(status: 0, message: "The relay returned no avatar URL")
    }

    /// The bytes of a blob I may read (a generated preview in my own slot), for copying.
    func blobData(projectId: Id, key: String) async throws -> Data {
        guard let url = URL(string: "\(endpoint.httpUrl)/blob/\(projectId)/\(key)") else {
            throw RelayError(status: 0, message: "Bad relay URL")
        }
        var req = URLRequest(url: url)
        req.setValue("Bearer \(token())", forHTTPHeaderField: "Authorization")
        let (body, response) = try await session.data(for: req)
        guard let http = response as? HTTPURLResponse, http.statusCode < 400 else {
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            throw RelayError.from(status: status, body: body)
        }
        return body
    }

    /// How long one avatar generation may take on the wire: past the relay's 75 s budget (40 s
    /// per provider attempt), so the relay's own 504 reaches the sheet before this client gives up
    /// (Jacob, 2026-10-26: the session's 30 s had cut a slow generation first).
    static let avatarGenerateTimeout: TimeInterval = 90

    private func generateAvatar(path: String, prompt: String) async throws -> String {
        let (data, _) = try await request("POST", path, body: ["prompt": prompt], timeout: Self.avatarGenerateTimeout)
        let v = (try? JSONSerialization.jsonObject(with: data)) ?? NSNull()
        guard let d = v as? [String: Any], let key = d["image_key"] as? String, !key.isEmpty else {
            throw RelayError(status: 0, message: "Couldn't generate the image.", detail: "The relay sent no picture")
        }
        return key
    }

    /// §11 `PATCH /projects/{id} {"vision_enabled": bool}` — may the agent sample the camera on calls.
    func setProjectVision(_ projectId: Id, enabled: Bool) async throws -> Bool? {
        try await decode(ProjectDto.self, "PATCH", "/projects/\(projectId)", body: ["vision_enabled": enabled]).vision_enabled
    }

    /// §10.3 `PATCH /projects/{id} {"voice": id|null}` (null = the default voice). Returns the
    /// project's `voice` as the relay now has it.
    func setProjectVoice(_ projectId: Id, voice: String?) async throws -> String? {
        let value: Any = voice.map { $0 as Any } ?? NSNull()
        let body: [String: Any] = ["voice": value]
        let kept = try await decode(ProjectDto.self, "PATCH", "/projects/\(projectId)", body: body).voice
        return kept.flatMap { $0.isEmpty ? nil : $0 }
    }

    /// §10.3 `GET /voices` (no auth) → the default id and the catalogue. `sample_url` may be
    /// relative; it is resolved against the relay.
    func voices() async throws -> Arbos.VoiceCatalog {
        let (data, _) = try await request("GET", "/voices", auth: false)
        let dto: VoiceCatalogDto
        do {
            dto = try JSONDecoder().decode(VoiceCatalogDto.self, from: data)
        } catch {
            NSLog("VoiceCatalogDto: \(error)")
            throw RelayError(status: 0, message: "The relay returned no voices")
        }
        let base = endpoint.httpUrl
        let list: [Arbos.Voice] = dto.voices.compactMap { v in
            guard !v.id.isEmpty else { return nil }
            let rawSample = v.sample_url.isEmpty ? "/voices/\(v.id)/sample.mp3" : v.sample_url
            let sample = rawSample.hasPrefix("http") ? rawSample : base + (rawSample.hasPrefix("/") ? "" : "/") + rawSample
            return Arbos.Voice(
                id: v.id,
                name: v.name.isEmpty ? v.id.capitalized : v.name,
                description: v.description,
                seconds: v.seconds,
                sampleURL: URL(string: sample)
            )
        }
        let defaultId = dto.`default`.isEmpty ? (list.first?.id ?? "") : dto.`default`
        return Arbos.VoiceCatalog(defaultId: defaultId, voices: list)
    }

    func peopleChats() async throws -> [PeopleChat] {
        try await peopleChatsJSON().compactMap(PeopleChat.parse)
    }

    /// The relay's `GET /chats` array as received, so the app can keep it on device.
    func peopleChatsJSON() async throws -> [Any] {
        (try await json("GET", "/chats")) as? [Any] ?? []
    }

    func peopleChat(_ id: String) async throws -> PeopleChat {
        guard let chat = PeopleChat.parse(try await json("GET", "/chats/\(id)")) else { throw RelayError(status: 0, message: "Bad chat") }
        return chat
    }

    func markRead(chatId: String, seq: Int64) async throws {
        _ = try await request("PUT", "/chats/\(chatId)/read", body: ["seq": seq])
    }

    /// `PUT /chats/{id}/mentions/read {message_id}` — the viewer has seen that mention. The
    /// relay resolves the row from the CRDT message id (no client ever sees a
    /// `chat_events.id`) and moves its mention cursor, which only ever goes forward.
    ///
    /// **It takes only the chat's oldest unread mention, so call it one at a time, oldest
    /// first.** The cursor is a watermark: honouring a newer id would swallow the older
    /// mentions under it, so the relay makes anything else a no-op. It is still a 204, not an
    /// error, because two devices marking the same mention and a client re-sending on
    /// foreground are both normal — which means a caller that names the wrong one gets a
    /// success back and a count that returns on the next `GET /me/unread`. The ordered walk
    /// is `AppModel.walkMentionReads`.
    ///
    /// Deliberately **not** `clear_on_read`. That payload makes every other device run
    /// `AppModel.clearingChat`, which zeroes the chat's unread **message** count — so clearing
    /// a mention here would wrongly mark the chat's messages read elsewhere. Other devices
    /// pick the cleared mention up on their next `GET /me/unread` instead.
    func markMentionsRead(chatId: String, messageId: String) async throws {
        _ = try await request("PUT", "/chats/\(chatId)/mentions/read", body: ["message_id": messageId])
    }

    /// Announce a message we just wrote so recipients get a real preview
    /// (§7c): people chats `POST /chats/{id}/notify`, project chats
    /// `POST /projects/{pid}/chats/{cid}/notify`. Deduped by the relay.
    func notifyMessage(chatId: String, projectId: Id?, messageId: String, text: String,
                       replyRef: ReplyRef? = nil) async throws {
        var body: [String: Any] = ["message_id": messageId, "text": String(text.prefix(140))]
        if let r = replyRef {
            // Reply in DM (#1834): the push says what the DM answers; the relay keeps the
            // address only for recipients who are members of the source chat.
            body["reply_ref"] = ["chat_id": r.chatId, "project_id": r.projectId, "message_id": r.messageId,
                                 "chat_title": r.chatTitle, "sender_name": r.who, "text": r.text]
        }
        if let projectId, projectId != peopleSlot {
            _ = try await request("POST", "/projects/\(projectId)/chats/\(chatId)/notify", body: body)
        } else {
            _ = try await request("POST", "/chats/\(chatId)/notify", body: body)
        }
    }

    func leaveChat(_ id: String) async throws { _ = try await request("POST", "/chats/\(id)/leave", body: [String: Any]()) }
    func hideChat(_ id: String) async throws { _ = try await request("POST", "/chats/\(id)/hide", body: [String: Any]()) }
    func deleteChat(_ id: String) async throws { _ = try await request("DELETE", "/chats/\(id)") }

    func addAgent(chatId: String, projectId: Id) async throws -> PeopleChat {
        let v = try await json("POST", "/chats/\(chatId)/agents", body: ["project_id": projectId])
        if let chat = PeopleChat.parse(v) { return chat }
        return try await peopleChat(chatId)
    }

    func removeAgent(chatId: String, projectId: Id) async throws {
        _ = try await request("DELETE", "/chats/\(chatId)/agents/\(projectId)")
    }

    func setReplyPolicy(chatId: String, projectId: Id, policy: String) async throws {
        _ = try await request("PUT", "/chats/\(chatId)/agents/\(projectId)/settings", body: ["reply_policy": policy])
    }

    func addMember(chatId: String, handle: String) async throws -> PeopleChat {
        let v = try await json("POST", "/chats/\(chatId)/members", body: ["handle": handle.trimmingCharacters(in: CharacterSet(charactersIn: "@ "))])
        if let chat = PeopleChat.parse(v) { return chat }
        return try await peopleChat(chatId)
    }

    /// One call for a batch of people and agents: the relay writes one membership notice (#1349).
    func addMembers(chatId: String, handles: [String], agentIds: [Id]) async throws -> PeopleChat {
        let cleaned = handles.map { $0.trimmingCharacters(in: CharacterSet(charactersIn: "@ ")) }.filter { !$0.isEmpty }
        let v = try await json("POST", "/chats/\(chatId)/members", body: ["handles": cleaned, "agent_ids": agentIds])
        if let chat = PeopleChat.parse(v) { return chat }
        return try await peopleChat(chatId)
    }

    /// `DELETE /chats/{id}/members/{userId}[?ban=1]` — owner/admin remove or ban.
    func removeMember(chatId: String, userId: String, ban: Bool = false) async throws {
        _ = try await request("DELETE", "/chats/\(chatId)/members/\(userId)\(ban ? "?ban=1" : "")")
    }

    /// `POST /chats/{id}/members/{userId}/unban` — clear a ban so they can be added again.
    func unbanMember(chatId: String, userId: String) async throws -> PeopleChat {
        let v = try await json("POST", "/chats/\(chatId)/members/\(userId)/unban")
        if let chat = PeopleChat.parse(v) { return chat }
        return try await peopleChat(chatId)
    }

    /// `PUT /chats/{id}/members/{userId}` — mute/unmute from posting (optional duration).
    func setMemberSilenced(
        chatId: String,
        userId: String,
        silenced: Bool,
        silencedUntilMs: Int64? = nil
    ) async throws -> PeopleChat {
        var body: [String: Any] = ["silenced": silenced]
        if silenced, let until = silencedUntilMs {
            body["silenced_until"] = until
        }
        let v = try await json("PUT", "/chats/\(chatId)/members/\(userId)", body: body)
        if let chat = PeopleChat.parse(v) { return chat }
        return try await peopleChat(chatId)
    }

    /// Owner: set a member's role to `admin` or `member`.
    func setMemberRole(chatId: String, userId: String, role: String) async throws -> PeopleChat {
        let v = try await json("PUT", "/chats/\(chatId)/members/\(userId)", body: ["role": role])
        if let chat = PeopleChat.parse(v) { return chat }
        return try await peopleChat(chatId)
    }

    /// Owner: whether this admin may mute and ban members.
    func setMemberCanMuteBan(chatId: String, userId: String, canMuteBan: Bool) async throws -> PeopleChat {
        let v = try await json("PUT", "/chats/\(chatId)/members/\(userId)", body: ["can_mute_ban": canMuteBan])
        if let chat = PeopleChat.parse(v) { return chat }
        return try await peopleChat(chatId)
    }

    /// `GET /chats/{id}/invites` — group invite links (same shape as server invites).
    func groupInvites(_ chatId: String) async throws -> [ServerInviteLink] {
        let v = try await json("GET", "/chats/\(chatId)/invites")
        let list = ((v as? [String: Any])?["invites"] as? [Any]) ?? []
        return list.compactMap(ServerInviteLink.parse)
    }

    /// `POST /chats/{id}/invites {expires, max_uses}` → the new group invite link.
    func createGroupInvite(_ chatId: String, expires: Bool, maxUses: Int?) async throws -> ServerInviteLink? {
        var body: [String: Any] = ["expires": expires]
        if let maxUses { body["max_uses"] = maxUses }
        return ServerInviteLink.parse(try await json("POST", "/chats/\(chatId)/invites", body: body))
    }

    /// `DELETE /chats/{id}/invites/{iid}` — the link stops working at once.
    func revokeGroupInvite(_ chatId: String, inviteId: String) async throws {
        _ = try await request("DELETE", "/chats/\(chatId)/invites/\(inviteId)")
    }

    /// `POST /chats/join {code}` (the code or the whole link) → the people group chat.
    func joinGroup(code: String) async throws -> PeopleChat {
        let v = try await json("POST", "/chats/join", body: ["code": code.trimmingCharacters(in: .whitespacesAndNewlines)])
        guard let chat = PeopleChat.parse(v) else { throw RelayError(status: 0, message: "The relay returned no chat") }
        return chat
    }

    /// Delete a message for everyone: the relay checks who may (403 `not_yours`) and writes
    /// the deletion into the doc itself. `room` is `people/<chat>` or `<project>/<chat>`.
    func deleteMessage(room: String, messageId: String) async throws {
        _ = try await request("DELETE", "/room/\(room)/messages/\(messageId)")
    }

    /// Remove a message's link preview for everyone (#2064): the relay checks who may (the
    /// delete rule, 403 `not_yours`), writes `preview: "off"` and drops the part itself.
    func removeLinkPreview(room: String, messageId: String) async throws {
        _ = try await request("DELETE", "/room/\(room)/messages/\(messageId)/preview")
    }

    /// `PUT /chats/{id}/settings {read_receipts?, muted?, archived?}` per member.
    func chatSettings(chatId: String, muted: Bool? = nil, readReceipts: Bool? = nil, archived: Bool? = nil) async throws {
        var body: [String: Any] = [:]
        if let muted { body["muted"] = muted }
        if let readReceipts { body["read_receipts"] = readReceipts }
        if let archived { body["archived"] = archived }
        _ = try await request("PUT", "/chats/\(chatId)/settings", body: body)
    }

    /// D92 amendment: a group's leaf picture from the avatar picker — `PATCH /chats/{id}
    /// {avatar_leaf, avatar_leaf_color, image_key: ""}` (nil = back to initials; the empty key
    /// clears the uploaded picture so the leaf shows).
    func setPeopleChatLeaf(_ chatId: String, pick: LeafPick?) async throws -> PeopleChat {
        let body: [String: Any] = [
            "avatar_leaf": pick.map { Int($0.leaf) as Any } ?? NSNull(),
            "avatar_leaf_color": pick.map { Int($0.color) as Any } ?? NSNull(),
            "image_key": "",
        ]
        let v = try await json("PATCH", "/chats/\(chatId)", body: body)
        guard let chat = PeopleChat.parse(v) else { throw RelayError(status: 0, message: "The relay returned no chat") }
        return chat
    }

    /// `PATCH /chats/{id} {title?, image_key?, description?}` — custom group name, picture key, about text.
    func patchPeopleChat(_ chatId: String, title: String? = nil, imageKey: String? = nil, description: String? = nil) async throws -> PeopleChat {
        var body: [String: Any] = [:]
        if let title { body["title"] = title }
        if let imageKey { body["image_key"] = imageKey }
        if let description { body["description"] = description }
        let v = try await json("PATCH", "/chats/\(chatId)", body: body)
        guard let chat = PeopleChat.parse(v) else { throw RelayError(status: 0, message: "The relay returned no chat") }
        return chat
    }

    func block(handle: String) async throws { _ = try await request("POST", "/blocks", body: ["handle": handle]) }
    func unblock(userId: String) async throws { _ = try await request("DELETE", "/blocks/\(userId)") }
    func blocks() async throws -> [Person] {
        try await decode([UserCardDto].self, "GET", "/blocks").map { card -> Person in
            var person = Person(dto: card)
            person.status = "blocked"
            return person
        }
    }

    func pushes() async throws -> [PushItem] {
        let list = (try await json("GET", "/me/pushes")) as? [Any] ?? []
        var out: [PushItem] = []
        for raw in list {
            if let item = PushItem.parse(raw) {
                out.append(item)
            } else {
                let kind = (raw as? [String: Any])?["kind"] as? String ?? "?"
                NSLog("PushItem: skipped unknown kind \(kind)")
            }
        }
        return out
    }

    // MARK: Connectors (F1)

    /// `GET /connectors/catalog` + `GET /connectors` → one view for Settings.
    func connectors() async throws -> ConnectorsView {
        let catalog = try await decode(ConnectorsCatalogDto.self, "GET", "/connectors/catalog")
        let list = try await decode(ConnectorsListDto.self, "GET", "/connectors")
        return ConnectorsView(catalog: catalog, list: list)
    }

    /// Start OAuth (`authUrl`) or store a pasted token. Google uses OAuth only.
    func connectConnector(provider: String, token: String? = nil) async throws -> String? {
        var body: [String: Any] = [:]
        if let token, !token.isEmpty { body["token"] = token }
        return try await decode(ConnectConnectorReplyDto.self, "POST", "/connectors/\(provider)/connect", body: body).authUrl
    }

    func disconnectConnector(id: String) async throws {
        _ = try await request("DELETE", "/connectors/\(id)")
    }

    // Projects together (step 3)
    func projectMembers(_ projectId: Id) async throws -> [ProjectMember] {
        try await decode(ProjectMembersDto.self, "GET", "/projects/\(projectId)/members").members.map(ProjectMember.init(dto:))
    }

    func addProjectMember(_ projectId: Id, handle: String) async throws -> [ProjectMember] {
        let body = ["handle": handle.trimmingCharacters(in: CharacterSet(charactersIn: "@ "))]
        return try await decode(ProjectMembersDto.self, "POST", "/projects/\(projectId)/members", body: body).members.map(ProjectMember.init(dto:))
    }

    func removeProjectMember(_ projectId: Id, userId: String) async throws {
        _ = try await request("DELETE", "/projects/\(projectId)/members/\(userId)")
    }

    /// Room checkpoint (crdt-sync §7.3). Returns `(seq, bytes)`.
    func checkpoint(room: String) async throws -> (Int64, Data) {
        let (data, http) = try await request("GET", "/room/\(room)/checkpoint")
        let seq = Int64(http.value(forHTTPHeaderField: "x-arbos-checkpoint-seq") ?? "") ?? 0
        return (seq, data)
    }

    /// WebSocket URL for a room; the token is re-read before every dial (§10).
    func roomURL(room: String) -> URL? {
        URL(string: "\(endpoint.wsUrl)/room/\(room)?token=\(token())")
    }

    // MARK: Goals (long-running objectives)

    /// Decode a goals list body; free-form plan/log come from the raw array elements.
    private static func parseGoalsList(_ data: Data) throws -> [GoalItem] {
        let list = try JSONDecoder().decode(GoalsListDto.self, from: data)
        let raw = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        let rawGoals = (raw?["goals"] as? [Any]) ?? []
        if rawGoals.count == list.goals.count {
            return zip(list.goals, rawGoals).map { dto, raw in
                GoalItem(dto: dto, raw: raw as? [String: Any] ?? [:])
            }
        }
        return list.goals.map { GoalItem(dto: $0) }
    }

    private static func parseGoal(_ data: Data) throws -> GoalItem {
        let dto = try JSONDecoder().decode(GoalDto.self, from: data)
        let raw = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
        return GoalItem(dto: dto, raw: raw)
    }

    /// `GET /goals` — every goal across the caller's projects.
    func goals() async throws -> [GoalItem] {
        let (data, _) = try await request("GET", "/goals")
        do {
            return try Self.parseGoalsList(data)
        } catch {
            NSLog("GoalsListDto: \(error)")
            throw RelayError(status: 0, message: "Bad goals reply")
        }
    }

    func projectGoals(_ projectId: Id) async throws -> [GoalItem] {
        let (data, _) = try await request("GET", "/projects/\(projectId)/goals")
        do {
            return try Self.parseGoalsList(data)
        } catch {
            NSLog("GoalsListDto: \(error)")
            throw RelayError(status: 0, message: "Bad goals reply")
        }
    }

    func createGoal(projectId: Id, _ body: [String: Any]) async throws -> GoalItem {
        let (data, _) = try await request("POST", "/projects/\(projectId)/goals", body: body)
        do {
            return try Self.parseGoal(data)
        } catch {
            NSLog("GoalDto: \(error)")
            throw RelayError(status: 0, message: "Bad goal reply")
        }
    }

    func patchGoal(projectId: Id, goalId: String, _ body: [String: Any]) async throws -> GoalItem {
        let (data, _) = try await request("PATCH", "/projects/\(projectId)/goals/\(goalId)", body: body)
        do {
            return try Self.parseGoal(data)
        } catch {
            NSLog("GoalDto: \(error)")
            throw RelayError(status: 0, message: "Bad goal reply")
        }
    }

    func deleteGoal(projectId: Id, goalId: String) async throws {
        _ = try await request("DELETE", "/projects/\(projectId)/goals/\(goalId)")
    }

    func appendGoalLog(projectId: Id, goalId: String, text: String) async throws -> GoalItem {
        let (data, _) = try await request(
            "POST", "/projects/\(projectId)/goals/\(goalId)/log", body: ["text": text]
        )
        do {
            return try Self.parseGoal(data)
        } catch {
            NSLog("GoalDto: \(error)")
            throw RelayError(status: 0, message: "Bad goal reply")
        }
    }
}

// MARK: - Outside agents (#2142)

/// The connection of an outside agent (`GET /outside-agents/{id}` → `agent`).
struct OutsideAgentInfo: Equatable {
    /// `connected` | `away` | `never` | `revoked`.
    var status: String
    var connected: Bool
    /// `acp` | `grok` | `custom`.
    var transport: String
    /// `asp/1`.
    var proto: String
    var lastSeenAt: Int64?
    /// The nine switches (spec §4.3), effective values: `"files.read": true`.
    var scopes: [String: Bool]

    /// The switches of the Can card in the mockup's order: wire name, title, subtitle.
    static let scopeRows: [(name: String, title: String, subtitle: String?)] = [
        ("chat.read", "Read messages", "in chats it is a member of"),
        ("chat.post", "Post, edit and delete its own messages", nil),
        ("chat.react", "React", nil),
        ("presence", "Show typing and status", nil),
        ("approval.request", "Ask you to approve actions", "approval cards in chat"),
        ("secret.use", "Use your secrets", "by name only, never the value"),
        ("members.manage", "Add members or agents", nil),
        ("files.read", "Open files shared in chat", "pictures, files, GIFs it was sent"),
        ("calls.join", "Join voice calls", nil),
    ]
    /// The Wake URL (`agent.wake`): the URL is shown back, the Authorization value never is.
    var wakeURL: String?
    var wakeAuthSet: Bool
    var wakeLast: OutsideWakeLast?
    /// True on a relay that has the Wake URL at all.
    var hasWake: Bool
    /// The session log's header numbers (`log` beside `agent` on the record, #2224); nil on a
    /// relay without the log — the card stays away.
    var logSummary: OutsideLogSummary? = nil

    init?(json: Any?, log: Any? = nil) {
        guard let d = json as? [String: Any] else { return nil }
        logSummary = OutsideLogSummary(json: log)
        status = d["status"] as? String ?? "never"
        connected = d["connected"] as? Bool ?? false
        transport = d["transport"] as? String ?? "custom"
        proto = d["protocol"] as? String ?? "asp/1"
        if let t = d["last_seen_at"] as? NSNumber { lastSeenAt = t.int64Value } else { lastSeenAt = nil }
        scopes = (d["scopes"] as? [String: Any])?.compactMapValues { $0 as? Bool } ?? [:]
        let wake = d["wake"] as? [String: Any]
        hasWake = wake != nil
        wakeURL = (wake?["url"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        wakeAuthSet = wake?["auth_set"] as? Bool ?? false
        wakeLast = OutsideWakeLast(json: wake?["last"])
    }

    /// A switch's value; a relay without scopes yet reads as the spec defaults.
    func scope(_ name: String) -> Bool {
        if let v = scopes[name] { return v }
        return ["chat.read", "chat.post", "chat.react", "presence", "approval.request"].contains(name)
    }

    /// The transport as the Details pane names it.
    var transportLabel: String {
        switch transport {
        case "acp": return "ACP · on your Mac"
        case "grok": return "Grok · HTTPS"
        default: return "Custom · HTTPS / WebSocket"
        }
    }

    /// "Connected · 12 s ago", "Away · 40 min ago", "Waiting for the first heartbeat", "Disconnected".
    func statusLine(now: Int64 = Int64(Date().timeIntervalSince1970 * 1000)) -> String {
        func ago(_ t: Int64) -> String {
            let s = max(0, (now - t) / 1000)
            if s < 60 { return "\(s) s ago" }
            if s < 3600 { return "\(s / 60) min ago" }
            if s < 86_400 { return "\(s / 3600) h ago" }
            return "\(s / 86_400) d ago"
        }
        switch (status, lastSeenAt) {
        case ("revoked", _): return "Disconnected"
        case ("connected", let t?): return "Connected · \(ago(t))"
        case ("connected", nil): return "Connected"
        case ("away", let t?): return "Away · \(ago(t))"
        case ("away", nil): return "Away"
        default: return "Waiting for the first heartbeat"
        }
    }
}

/// `log {entries, last_at, last_reply_ms}` on a connected agent's record (#2224).
struct OutsideLogSummary: Equatable {
    var entries: Int
    var lastAt: Int64?
    var lastReplyMs: Int64?

    init?(json: Any?) {
        guard let d = json as? [String: Any] else { return nil }
        entries = (d["entries"] as? NSNumber)?.intValue ?? 0
        lastAt = (d["last_at"] as? NSNumber)?.int64Value
        lastReplyMs = (d["last_reply_ms"] as? NSNumber)?.int64Value
    }
}

/// One line of a connected agent's session log as the sidecar uploaded it (spec §13.3): a wake,
/// an ack, a reply, a session event or an error — never message text.
struct OutsideLogEntry: Equatable, Identifiable {
    let id: Int64
    let at: Int64
    let kind: String
    let chat: String?
    let message: String?
    let ms: [String: Int64]
    let session: String?
    let error: String?
    let turns: Int?
    let reason: String?
    let handoffFrom: String?

    init?(json: Any?) {
        guard let d = json as? [String: Any], let kind = d["kind"] as? String else { return nil }
        id = (d["id"] as? NSNumber)?.int64Value ?? 0
        at = (d["at"] as? NSNumber)?.int64Value ?? 0
        self.kind = kind
        chat = d["chat"] as? String
        message = d["message"] as? String
        ms = (d["ms"] as? [String: Any])?.compactMapValues { ($0 as? NSNumber)?.int64Value } ?? [:]
        session = d["session"] as? String
        error = d["error"] as? String
        turns = (d["turns"] as? NSNumber)?.intValue
        reason = d["reason"] as? String
        handoffFrom = d["handoff_from"] as? String
    }

    var isError: Bool { kind == "error" }
    var isSession: Bool { kind.hasPrefix("session.") }
    /// Rows worth a line on the card: replies, errors and session events; wakes, typing, reads
    /// and uploads are the plumbing behind them.
    var shown: Bool { kind == "reply" || kind == "post" || isError || isSession }

    private static func secs(_ ms: Int64) -> String { String(format: "%.1f s", Double(ms) / 1000) }

    /// The first line for a session event.
    var sessionTitle: String {
        switch kind {
        case "session.new": return "new session"
        case "session.resume": return "session resumed"
        case "session.resume_failed": return "session could not be resumed"
        case "session.rotate": return "session rotated"
        default: return "session"
        }
    }

    /// The muted second line: "ack 1.2 s · reply 9.8 s", "turn 12", or the error sentence.
    var detail: String {
        if let error { return error }
        if isSession {
            switch kind {
            case "session.resume": return turns.map { "turn \($0)" } ?? ""
            case "session.new": return handoffFrom.map { "handoff from \($0.prefix(4))…" } ?? "first session"
            case "session.rotate": return reason ?? ""
            default: return ""
            }
        }
        var parts: [String] = []
        if let a = ms["event_to_ack"] { parts.append("ack \(Self.secs(a))") }
        if let r = ms["event_to_reply"] ?? ms["nudge_to_reply"] { parts.append("reply \(Self.secs(r))") }
        return parts.joined(separator: " · ")
    }

    var date: Date { Date(timeIntervalSince1970: Double(at) / 1000) }

    /// "17:02:11" in local time.
    var clock: String {
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss"
        return f.string(from: date)
    }

    /// "Today", "Yesterday", or the weekday and date — the View all page's section headers.
    var day: String {
        let cal = Calendar.current
        if cal.isDateInToday(date) { return "Today" }
        if cal.isDateInYesterday(date) { return "Yesterday" }
        let f = DateFormatter()
        f.dateFormat = "EEEE, d MMMM"
        return f.string(from: date)
    }
}

/// The last nudge the relay sent to an outside agent's Wake URL.
struct OutsideWakeLast: Equatable {
    var at: Int64
    var status: Int?
    var error: String?

    init?(json: Any?) {
        guard let d = json as? [String: Any], let at = d["at"] as? NSNumber else { return nil }
        self.at = at.int64Value
        status = (d["status"] as? NSNumber)?.intValue
        error = (d["error"] as? String).flatMap { $0.isEmpty ? nil : $0 }
    }

    init(at: Int64, status: Int?, error: String?) {
        self.at = at; self.status = status; self.error = error
    }

    /// "Delivered · HTTP 200 · 12 s ago" / "Failed · HTTP 500: boom · 3 min ago" (the Mac's `WakeLast::line`).
    func line(now: Int64 = Int64(Date().timeIntervalSince1970 * 1000)) -> String {
        let s = max(0, (now - at) / 1000)
        let when: String
        if s < 60 { when = "\(s) s ago" } else if s < 3600 { when = "\(s / 60) min ago" } else if s < 86_400 { when = "\(s / 3600) h ago" } else { when = "\(s / 86_400) d ago" }
        if let error { return "Failed · \(error) · \(when)" }
        if let status { return "Delivered · HTTP \(status) · \(when)" }
        return "Delivered · \(when)"
    }
}

/// What the reveal screen shows once: the key and the link, plus the live connection.
struct ConnectReveal: Equatable {
    var projectId: Id
    var name: String
    var transport: String
    var key: String
    var link: String
    var relayURL: String
    var chatId: Id?
    var rotated: Bool
    var info: OutsideAgentInfo?

    init?(json: Any, rotated: Bool) {
        guard let d = json as? [String: Any], let agent = d["agent"] as? [String: Any],
              let id = agent["project_id"] as? String, let key = d["key"] as? String else { return nil }
        projectId = id
        name = agent["name"] as? String ?? ""
        transport = agent["transport"] as? String ?? "custom"
        self.key = key
        link = d["link"] as? String ?? ""
        relayURL = d["relay_url"] as? String ?? ""
        chatId = d["chat_id"] as? String
        self.rotated = rotated
        info = OutsideAgentInfo(json: agent)
    }
}
