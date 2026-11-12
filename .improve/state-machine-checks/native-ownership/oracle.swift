import Foundation

func observation(_ accepted: [Bool], _ issued: Int?, _ current: Bool, other: Bool? = nil) -> [String: Any] {
    var out: [String: Any] = ["accepted": accepted, "issued": issued.map { $0 as Any } ?? NSNull(), "hasCurrent": current]
    if let other { out["other"] = other }
    return out
}

func run(_ value: [String: Any]) -> [String: Any]? {
    guard let id = value["id"] as? Int, let machine = value["machine"] as? String else { return nil }
    if machine == "snapshot" {
        guard let p = value["snapshot"] as? [String: Any], let closed = p["closed"] as? Bool,
              let foreign = p["foreign"] as? Bool, let disposed = p["disposed"] as? Bool,
              let sameDoc = p["sameDoc"] as? Bool, let empty = p["empty"] as? Bool,
              let revision = p["revision"] as? Int, let latest = p["latest"] as? Int else { return nil }
        var account = WorkLifetime()
        let other = WorkLifetime()
        let docs = [NSObject(), NSObject()]
        let ticket = SnapshotPublication.Ticket(account: foreign ? other.ticket! : account.ticket!,
            doc: ObjectIdentifier(docs[0]), revision: revision)
        if closed { account.close() }
        let target: SnapshotPublication.Target = disposed ? .disposed : .active(doc: ObjectIdentifier(docs[sameDoc ? 0 : 1]), revision: latest)
        return ["id": id, "accepted": SnapshotPublication.accepts(ticket, account: account, target: target, empty: empty)]
    }
    guard let actions = value["actions"] as? [[String: Any]] else { return nil }
    var steps: [[String: Any]] = []
    if machine == "work" {
        var owner = WorkLifetime()
        let other = WorkLifetime().ticket!
        var tickets = [owner.ticket!]
        func observe(_ issued: Int? = nil) -> [String: Any] {
            observation(tickets.map { owner.accepts($0) }, issued, owner.ticket != nil, other: owner.accepts(other))
        }
        let initial = observe()
        for action in actions {
            guard let kind = action["kind"] as? String else { return nil }
            var issued: Int?
            switch kind {
            case "close": owner.close()
            case "reopen": tickets.append(owner.reopen()); issued = tickets.count - 1
            case "capture": if let ticket = owner.ticket { tickets.append(ticket); issued = tickets.count - 1 }
            default: return nil
            }
            steps.append(observe(issued))
        }
        return ["id": id, "initial": initial, "steps": steps]
    }
    if machine == "matrix" {
        let account = WorkLifetime().ticket!
        var owner = MatrixLifetime()
        var tickets: [MatrixLifetime.Ticket] = []
        func observe(_ issued: Int? = nil) -> [String: Any] {
            let accepted = tickets.map { owner.accepts($0) }
            return observation(accepted, issued, accepted.contains(true))
        }
        let initial = observe()
        for action in actions {
            guard let kind = action["kind"] as? String else { return nil }
            var issued: Int?
            switch kind {
            case "start": if let ticket = owner.start(account: account) { tickets.append(ticket); issued = tickets.count - 1 }
            case "stop": owner.stop()
            case "ready", "failed":
                guard let index = action["index"] as? Int, tickets.indices.contains(index) else { return nil }
                if kind == "ready" { owner.ready(tickets[index]) } else { owner.failed(tickets[index]) }
            default: return nil
            }
            steps.append(observe(issued))
        }
        return ["id": id, "initial": initial, "steps": steps]
    }
    return nil
}

@main struct Oracle {
    static func main() {
        while let line = readLine() {
            let input = line.data(using: .utf8)!
            let parsed = try? JSONSerialization.jsonObject(with: input) as? [String: Any]
            let output = parsed.flatMap(run) ?? ["id": NSNull(), "error": "Invalid trace"]
            guard let bytes = try? JSONSerialization.data(withJSONObject: output, options: [.sortedKeys]),
                  String(data: bytes, encoding: .utf8) != nil else { exit(2) }
            FileHandle.standardOutput.write(bytes + Data([10]))
        }
    }
}
