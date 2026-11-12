import Foundation

@main struct DraftOracle {
    static func main() {
        while let line = readLine() {
            guard let data = line.data(using: .utf8),
                  let input = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let actions = input["actions"] as? [[String: Any]], let id = input["id"] as? Int else { return }
            let accounts = [WorkLifetime().ticket!, WorkLifetime().ticket!]
            var queue = DraftSendQueue<Int>(owner: accounts[0])
            var tickets: [DraftSendQueue<Int>.Ticket] = []
            var steps: [[String: Any]] = []
            for action in actions {
                var outcome = "none"
                var issued: Any = NSNull()
                var sends: Any = NSNull()
                switch action["kind"] as! String {
                case "enqueue":
                    let account = accounts[action["account"] as! Int]
                    switch queue.enqueue(draft: action["draft"] as! String, account: account,
                                         attempt: WorkLifetime().ticket!, send: action["value"] as! Int) {
                    case .started(let ticket):
                        outcome = "started"; issued = tickets.count; tickets.append(ticket)
                    case .queued(let ticket):
                        outcome = "queued"; issued = tickets.count; tickets.append(ticket)
                    case .refused: outcome = "refused"
                    }
                case "finish":
                    let index = action["index"] as! Int
                    if tickets.indices.contains(index), let values = queue.finish(tickets[index]) {
                        outcome = "finished"; sends = values
                    }
                case "retire": queue.retire(); outcome = "retired"
                case "fresh": queue = DraftSendQueue<Int>(owner: accounts[action["account"] as! Int]); outcome = "fresh"
                default: return
                }
                steps.append(["outcome": outcome, "issued": issued, "sends": sends,
                              "pending": [queue.isPending("a"), queue.isPending("b"), queue.isPending("")],
                              "owner": queue.owner.flatMap { accounts.firstIndex(of: $0) } as Any? ?? NSNull()])
            }
            let reply: [String: Any] = ["id": id, "steps": steps]
            guard let out = try? JSONSerialization.data(withJSONObject: reply), let text = String(data: out, encoding: .utf8) else { return }
            print(text)
            fflush(stdout)
        }
    }
}
