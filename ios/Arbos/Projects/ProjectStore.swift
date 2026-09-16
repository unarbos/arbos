import Foundation

/// One project the phone can open: a kernel, and the face it wears.
struct ProjectEntry: Identifiable, Equatable {
    let target: KernelTarget
    /// The folder's own name, what the tab reads with no name set.
    var folder: String
    var machine: String
    var place: String
    var live: Bool
    var identity: ProjectIdentity

    var id: String { target.stored }
    var title: String { identity.label ?? folder }
}

/// The projects list: the pod kernel, then every project the hub knows
/// about, in the order the desktop's tabs would show them. Faces come
/// from `.arbos/project.toml` (read once a chat attaches) and are kept
/// here so the list shows them before the next attach.
@MainActor
final class ProjectStore: ObservableObject {
    @Published private(set) var entries: [ProjectEntry] = []
    @Published private(set) var loading = false
    @Published private(set) var problem: String?

    private let settings: AppSettings
    private let defaults = UserDefaults.standard

    init(settings: AppSettings) {
        self.settings = settings
        entries = cachedEntries()
    }

    func refresh() async {
        loading = true
        problem = nil
        var list: [ProjectEntry] = []
        if settings.kernelEndpoint != nil {
            list.append(entry(target: .pod, folder: "pod", machine: "pod", place: "", live: true, remote: true))
        }
        if settings.hubConfigured {
            do {
                let machines = try await HubClient.list(hubURL: settings.hubURL, token: settings.hubToken)
                for machine in machines {
                    // `<project>--<child>` is a worker's worktree place, not a
                    // project of Jacob's: it belongs under its parent's chat.
                    for project in machine.projects where !project.name.contains("--") {
                        let target = KernelTarget.hub(machine: machine.name, project: project.name)
                        var row = entry(
                            target: target,
                            folder: project.name,
                            machine: machine.name,
                            place: project.place,
                            live: project.live,
                            remote: true
                        )
                        // The roster's face (#233) beats the cache and the default.
                        if let face = project.identity?.filled(key: target.stored) {
                            row.identity = face
                            remember(face, for: target)
                        }
                        list.append(row)
                    }
                }
            } catch {
                problem = error.localizedDescription
                #if DEBUG
                print("roster \(settings.hubURL): \(error)")
                #endif
            }
        }
        if list.isEmpty, entries.isEmpty {
            problem = problem ?? "No kernel or hub is set. Open Settings."
        }
        if !list.isEmpty { entries = list }
        saveCache()
        loading = false
    }

    /// The face a chat read off its kernel: keep it for the list.
    func remember(_ identity: ProjectIdentity, for target: KernelTarget) {
        if let data = try? JSONEncoder().encode(identity) {
            defaults.set(data, forKey: "identity:\(target.stored)")
        }
        if let index = entries.firstIndex(where: { $0.target == target }) {
            entries[index].identity = identity
        }
    }

    func identity(for target: KernelTarget, remote: Bool) -> ProjectIdentity {
        if let data = defaults.data(forKey: "identity:\(target.stored)"),
           let saved = try? JSONDecoder().decode(ProjectIdentity.self, from: data) {
            return saved
        }
        return ProjectIdentity.defaults(key: target.stored, remote: remote)
    }

    // MARK: - Private

    private func entry(target: KernelTarget, folder: String, machine: String, place: String, live: Bool, remote: Bool) -> ProjectEntry {
        ProjectEntry(
            target: target, folder: folder, machine: machine, place: place, live: live,
            identity: identity(for: target, remote: remote)
        )
    }

    private struct CachedEntry: Codable {
        var target: String
        var folder: String
        var machine: String
        var place: String
    }

    private func saveCache() {
        let rows = entries.map { CachedEntry(target: $0.target.stored, folder: $0.folder, machine: $0.machine, place: $0.place) }
        if let data = try? JSONEncoder().encode(rows) { defaults.set(data, forKey: "projects.cache") }
    }

    /// Last roster, so the list is not empty for the seconds the hub takes.
    private func cachedEntries() -> [ProjectEntry] {
        guard let data = defaults.data(forKey: "projects.cache"),
              let rows = try? JSONDecoder().decode([CachedEntry].self, from: data) else { return [] }
        return rows.map { row in
            let target = KernelTarget(stored: row.target)
            return entry(target: target, folder: row.folder, machine: row.machine, place: row.place, live: false, remote: true)
        }
    }
}
