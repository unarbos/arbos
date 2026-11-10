// Historical exact registry implementation, before bounded loading.
// Full RelayAPI source SHA256 0d50ec56055a25ad6cb7f595a679a03e33b14b9aeb7eacb887008ac592ca2985
struct RegistryProject {
    var project: Arbos.Project
    var chats: [Arbos.Chat]
}
struct RegistryResult {
    var projects: [RegistryProject]
    var skippedIds: Set<String>
    var incomplete: Bool
}
private struct ProjectListId: Decodable {
    var id: String
}
final class RelayAPI {
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
}
