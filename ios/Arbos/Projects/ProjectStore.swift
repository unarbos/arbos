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
    /// This project's kernel is running from a file that has been deleted.
    /// It answers, so it is not Off, but it refuses every worker it is asked
    /// to start — the state behind JB-6, which looked healthy from outside
    /// for days. A live fact from the roster, never cached.
    var needsRestart = false
    /// The roster no longer lists this project, but Jacob has opened it
    /// before, so the row stays and says what it is waiting on rather than
    /// disappearing. Empty when the project is live.
    var waitingOn: String = ""
    /// When this project's transcript last gained a line, from the hub
    /// (#538). Nil when the hub has heard nothing, and the row says nothing
    /// rather than guessing at "just now".
    var lastActivity: Date?

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
    private var retryTask: Task<Void, Never>?
    private var retryAttempt = 0

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
        #if DEBUG
        print("roster: hub \(settings.hubURL) configured=\(settings.hubConfigured) token=\(settings.hubToken.count) chars")
        #endif
        var hubAnswered = !settings.hubConfigured
        if settings.hubConfigured {
            do {
                let machines = try await HubClient.list(hubURL: settings.hubURL, token: settings.hubToken)
                hubAnswered = true
                rosterMachines = machines.map(\.name)
                #if DEBUG
                print("roster: \(machines.count) machines, \(machines.flatMap(\.projects).count) projects")
                #endif
                for machine in machines {
                    // The hub says what a place is (#346): a worker's worktree
                    // or a service (the feedback pipe) is not a project of
                    // Jacob's, whatever its name looks like.
                    for project in machine.projects where project.kind.isEmpty {
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
                        // This project's own kernel, not the machine: a
                        // machine runs several processes and they can be on
                        // different builds, which is why the hub stopped
                        // keeping one row for all of them (#385).
                        row.needsRestart = machine.build(forProject: project.name)?.binaryGone ?? false
                        if project.lastActivityMs > 0 {
                            row.lastActivity = Date(timeIntervalSince1970: Double(project.lastActivityMs) / 1000)
                        }
                        // A machine the hub is holding open after its last
                        // kernel left (#545). Its projects arrive `live:
                        // false` like any idle one, so without this the row
                        // reads "Off" — true, and silent about which thing
                        // is off, which is the only question this line has
                        // to answer: his machine, or that project's kernel.
                        if !machine.online {
                            row.waitingOn = "\(machine.name) is asleep"
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
        // A project he has opened before does not vanish because its machine
        // went off. It went quiet, which is a different thing and reads very
        // differently: the row disappearing says his work is gone, and a
        // stopped kernel says it is asleep. It is also the only way he ever
        // sees why — the refusal wording is unreachable when there is no row
        // to tap (M-151, M-168).
        if hubAnswered {
            let machines = Set(rosterMachines)
            for target in openedBefore where !list.contains(where: { $0.target == target }) {
                guard case .hub(let machine, let project) = target else { continue }
                var row = entry(target: target, folder: project, machine: machine,
                                place: "", live: false, remote: true)
                row.waitingOn = machines.contains(machine)
                    ? "\(project) isn't running on \(machine)"
                    : "\(machine) is off"
                #if DEBUG
                // Which path kept the row, and why it says what it says.
                // Cycle 51 could not tell this from the screen and guessed.
                print("roster: keeping \(target.stored) — \(row.waitingOn)")
                #endif
                list.append(row)
            }
        }
        if !hubAnswered {
            // The hub did not answer: its rows from last time stay, marked
            // off, instead of the list shrinking to the pod row and that
            // shrunken list being cached (M-88, seen after an offline cold start).
            for var row in entries where row.target != .pod && !list.contains(where: { $0.target == row.target }) {
                row.live = false
                list.append(row)
            }
        }
        fold(&list)
        if list.isEmpty, entries.isEmpty {
            problem = problem ?? "No kernel or hub is set. Open Settings."
        }
        if !list.isEmpty { entries = list }
        if hubAnswered { saveCache() }
        loading = false
        // Rows marked Off come back by themselves when the link does (M-92):
        // try again at 10, 20, 40, then every 60 s until the hub answers.
        retryTask?.cancel()
        retryTask = nil
        if hubAnswered {
            retryAttempt = 0
        } else {
            let delay = min(60, 10 << min(retryAttempt, 2))
            retryAttempt += 1
            retryTask = Task { [weak self] in
                try? await Task.sleep(for: .seconds(delay))
                guard let self, !Task.isCancelled else { return }
                await self.refresh()
            }
        }
    }

    /// The direct kernel said where it lives on the hub (`hello.store`,
    /// `arbos://<machine>/<project>/`). Since the ArbosLife cutover that is
    /// the same kernel the roster lists as `phone`, and the list drew it
    /// twice — once as "pod", once under its name (M-121). Kept, so the
    /// roster row stands for both from now on.
    func podIsAlso(_ address: String) {
        let path = address.replacingOccurrences(of: "arbos://", with: "")
        let parts = path.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
        guard parts.count >= 2 else { return }
        let twin = "\(parts[0])/\(parts[1])"
        guard twin != podTwin else { return }
        defaults.set(twin, forKey: "pod.twin")
        var list = entries
        fold(&list)
        entries = list
    }

    /// `<machine>/<project>` the pod row is another door to, once known.
    private var podTwin: String? { defaults.string(forKey: "pod.twin") }

    /// Machines the last roster listed, for telling "the machine is off"
    /// from "the machine is up and this project is not running".
    private var rosterMachines: [String] = []

    /// Projects Jacob has opened at least once. Only these keep a row when
    /// the roster stops listing them; everything he has never opened can
    /// come and go without cluttering his list.
    private var openedBefore: [KernelTarget] {
        (defaults.array(forKey: "projects.opened") as? [String] ?? []).map(KernelTarget.init(stored:))
    }

    /// Called when a project is opened, so its row survives its machine.
    func remember(opened target: KernelTarget) {
        guard case .hub = target else { return }
        var seen = defaults.array(forKey: "projects.opened") as? [String] ?? []
        guard !seen.contains(target.stored) else { return }
        seen.append(target.stored)
        defaults.set(Array(seen.suffix(40)), forKey: "projects.opened")
    }

    /// One project, one row: the pod row goes when its twin is in the list.
    private func fold(_ list: inout [ProjectEntry]) {
        guard let twin = podTwin,
              list.contains(where: { if case .hub(let m, let p) = $0.target { return "\(m)/\(p)" == twin } else { return false } })
        else { return }
        list.removeAll { $0.target == .pod }
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
