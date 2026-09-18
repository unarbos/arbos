import Foundation

/// One machine as `arbos-hub` lists it (`MachineInfo` in `arbos-core::hub`).
struct HubMachine: Decodable, Identifiable, Equatable {
    struct Project: Decodable, Identifiable, Equatable {
        var name: String
        var place: String
        /// A kernel serves it now; only these can be attached to.
        var live: Bool
        /// The face from the project's `project.toml`, as the hub relays it
        /// (#233); absent when nobody has read the file.
        var identity: ProjectIdentity?
        /// What the place says it is (#346): empty for a project of Jacob's;
        /// `worktree` for a worker's checkout, `service` for infrastructure
        /// (the feedback pipe). Only the empty kind is a row in the list.
        var kind: String
        /// When this project's transcript last gained a line, as the hub
        /// heard it (#538). Absent — and so zero — when the hub has heard
        /// nothing; the list shows nothing rather than "just now".
        var lastActivityMs: Int64

        var id: String { name }

        init(name: String, place: String = "", live: Bool = false, identity: ProjectIdentity? = nil, kind: String = "", lastActivityMs: Int64 = 0) {
            self.name = name
            self.place = place
            self.live = live
            self.identity = identity
            self.kind = kind
            self.lastActivityMs = lastActivityMs
        }

        // Fields the hub leaves out when empty must decode as defaults.
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            name = try c.decode(String.self, forKey: .name)
            place = try c.decodeIfPresent(String.self, forKey: .place) ?? ""
            live = try c.decodeIfPresent(Bool.self, forKey: .live) ?? false
            identity = try c.decodeIfPresent(ProjectIdentity.self, forKey: .identity)
            kind = try c.decodeIfPresent(String.self, forKey: .kind) ?? ""
            lastActivityMs = try c.decodeIfPresent(Int64.self, forKey: .lastActivityMs) ?? 0
        }

        private enum CodingKeys: String, CodingKey {
            case name, place, live, identity, kind
            case lastActivityMs = "last_activity_ms"
        }
    }

    /// One process registered from this machine — the worker daemon, or a
    /// kernel serving one project — with the build it reported for itself.
    struct Build: Decodable, Equatable {
        /// `worker` for the daemon, `kernel` for a project's kernel.
        var role: String
        /// The project a kernel serves; empty for the worker.
        var project: String
        var version: String
        var gitSha: String
        var builtAt: String
        /// The file this process started from is gone. It keeps serving and
        /// refuses every spawn, which is the shape of JB-6.
        var binaryGone: Bool

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            role = try c.decodeIfPresent(String.self, forKey: .role) ?? ""
            project = try c.decodeIfPresent(String.self, forKey: .project) ?? ""
            version = try c.decodeIfPresent(String.self, forKey: .version) ?? ""
            gitSha = try c.decodeIfPresent(String.self, forKey: .gitSha) ?? ""
            builtAt = try c.decodeIfPresent(String.self, forKey: .builtAt) ?? ""
            binaryGone = try c.decodeIfPresent(Bool.self, forKey: .binaryGone) ?? false
        }

        private enum CodingKeys: String, CodingKey {
            case role, project, version
            case gitSha = "git_sha"
            case builtAt = "built_at"
            case binaryGone = "binary_gone"
        }
    }

    var name: String
    var host: String
    var worker: Bool
    var projects: [Project]
    /// Every process on this machine, each with its own build.
    ///
    /// Deliberately not the roster's machine-level `git_sha`: the hub fills
    /// that only when every process agrees and leaves it empty when they
    /// differ, so empty means "they differ, read these" and not "unknown".
    /// A phone that showed the top-level field would go blank on exactly the
    /// machines whose build is worth knowing.
    var builds: [Build]
    /// Some process here runs a deleted file. `builds` says which.
    var binaryGone: Bool

    var id: String { name }

    /// The build of the kernel serving `project` — the process a person in
    /// that project is actually talking to, which is the only build that
    /// answers "what am I using?" on a machine running several.
    func build(forProject project: String) -> Build? {
        builds.first { $0.role == "kernel" && $0.project == project }
    }

    init(name: String, host: String = "", worker: Bool = false, projects: [Project] = [], builds: [Build] = [], binaryGone: Bool = false) {
        self.name = name
        self.host = host
        self.worker = worker
        self.projects = projects
        self.builds = builds
        self.binaryGone = binaryGone
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        host = try c.decodeIfPresent(String.self, forKey: .host) ?? ""
        worker = try c.decodeIfPresent(Bool.self, forKey: .worker) ?? false
        projects = try c.decodeIfPresent([Project].self, forKey: .projects) ?? []
        builds = try c.decodeIfPresent([Build].self, forKey: .builds) ?? []
        binaryGone = try c.decodeIfPresent(Bool.self, forKey: .binaryGone) ?? false
    }

    private enum CodingKeys: String, CodingKey {
        case name, host, worker, projects, builds
        case binaryGone = "binary_gone"
    }
}

/// Read-only view of the mesh hub (`crates/arbos-hub`): the roster from
/// `GET /list`, and the address a kernel is attached through.
///
/// `/attach/<machine>/<project>` relays plain kernel frames both ways, so
/// `ArbosKernelClient` speaks to a hub-routed kernel exactly as it speaks
/// to the pod kernel; only the URL and the token differ.
enum HubClient {
    static func list(hubURL: String, token: String) async throws -> [HubMachine] {
        guard var components = URLComponents(string: hubURL) else { throw HubError.badURL }
        components.scheme = components.scheme?.lowercased() == "ws" ? "http" : "https"
        components.path = "/list"
        components.queryItems = nil
        guard let url = components.url else { throw HubError.badURL }
        var request = URLRequest(url: url)
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.timeoutInterval = 10
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw HubError.badResponse }
        guard http.statusCode == 200 else { throw HubError.status(http.statusCode) }
        struct Roster: Decodable { var machines: [HubMachine] }
        do {
            return try JSONDecoder().decode(Roster.self, from: data).machines
                .sorted { $0.name < $1.name }
        } catch {
            throw HubError.badResponse
        }
    }

    /// The WebSocket a client attaches through for one machine's project.
    static func attachURL(hubURL: String, machine: String, project: String?) -> URL? {
        guard var components = URLComponents(string: hubURL) else { return nil }
        // A plain hub (http/ws — a machine on the same network) stays plain.
        components.scheme = ["http", "ws"].contains(components.scheme?.lowercased() ?? "") ? "ws" : "wss"
        var path = "/attach/\(machine)"
        if let project, !project.isEmpty { path += "/\(project)" }
        components.path = path
        components.queryItems = nil
        return components.url
    }
}

enum HubError: LocalizedError {
    case badURL
    case badResponse
    case status(Int)

    var errorDescription: String? {
        switch self {
        case .badURL: return "Bad hub URL."
        case .badResponse: return "The hub answered with something that is not a roster."
        case .status(let code): return code == 401 ? "Hub token refused." : "Hub answered \(code)."
        }
    }
}

/// Which kernel the main chat is attached to.
enum KernelTarget: Equatable, Hashable {
    /// The phone kernel on the pod (Settings → Arbos kernel).
    case pod
    /// A kernel reached by name through the hub.
    case hub(machine: String, project: String)

    var label: String {
        switch self {
        case .pod: return "pod"
        case .hub(let machine, let project): return project.isEmpty ? machine : "\(machine)/\(project)"
        }
    }

    /// The project's folder on its machine, which is what a person calls it
    /// when the machine is already understood — a chat header, a banner.
    /// `label` names the machine too and is for places where it is not.
    var folder: String? {
        switch self {
        case .pod: return nil
        case .hub(_, let project): return project.isEmpty ? nil : project
        }
    }

    var stored: String {
        switch self {
        case .pod: return "pod"
        case .hub(let machine, let project): return "hub:\(machine)/\(project)"
        }
    }

    init(stored: String) {
        if stored.hasPrefix("hub:") {
            let rest = stored.dropFirst("hub:".count)
            let parts = rest.split(separator: "/", maxSplits: 1).map(String.init)
            self = .hub(machine: parts.first ?? "", project: parts.count > 1 ? parts[1] : "")
        } else {
            self = .pod
        }
    }
}
