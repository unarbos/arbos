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

        var id: String { name }

        init(name: String, place: String = "", live: Bool = false, identity: ProjectIdentity? = nil) {
            self.name = name
            self.place = place
            self.live = live
            self.identity = identity
        }

        // Fields the hub leaves out when empty must decode as defaults.
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            name = try c.decode(String.self, forKey: .name)
            place = try c.decodeIfPresent(String.self, forKey: .place) ?? ""
            live = try c.decodeIfPresent(Bool.self, forKey: .live) ?? false
            identity = try c.decodeIfPresent(ProjectIdentity.self, forKey: .identity)
        }

        private enum CodingKeys: String, CodingKey { case name, place, live, identity }
    }

    var name: String
    var host: String
    var worker: Bool
    var projects: [Project]

    var id: String { name }

    init(name: String, host: String = "", worker: Bool = false, projects: [Project] = []) {
        self.name = name
        self.host = host
        self.worker = worker
        self.projects = projects
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        host = try c.decodeIfPresent(String.self, forKey: .host) ?? ""
        worker = try c.decodeIfPresent(Bool.self, forKey: .worker) ?? false
        projects = try c.decodeIfPresent([Project].self, forKey: .projects) ?? []
    }

    private enum CodingKeys: String, CodingKey { case name, host, worker, projects }
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
        components.scheme = components.scheme?.lowercased() == "http" ? "ws" : "wss"
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
