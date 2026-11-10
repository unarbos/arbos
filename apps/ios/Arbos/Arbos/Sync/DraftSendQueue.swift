/// First sends are coalesced only within one draft and account. Retired queues
/// cannot reopen; a new account receives a fresh queue and operation tickets.
struct DraftSendQueue<Send> {
    struct Ticket: Equatable {
        let account: WorkLifetime.Ticket
        let draft: String
        let attempt: WorkLifetime.Ticket
    }

    enum Enqueued {
        case started(Ticket)
        case queued(Ticket)
        case refused
    }

    private struct Pending {
        let ticket: Ticket
        var sends: [Send]
    }

    private enum State {
        case closed
        case open(WorkLifetime.Ticket, [String: Pending])
    }

    private enum Event {
        case enqueue(String, WorkLifetime.Ticket, WorkLifetime.Ticket, Send)
        case finish(Ticket)
        case retire
    }

    private enum Effect {
        case none
        case enqueued(Enqueued)
        case finished([Send])
    }

    private var state: State

    init() { state = .closed }
    init(owner: WorkLifetime.Ticket) { state = .open(owner, [:]) }

    var owner: WorkLifetime.Ticket? {
        guard case .open(let owner, _) = state else { return nil }
        return owner
    }

    func isPending(_ draft: String) -> Bool {
        guard case .open(_, let pending) = state else { return false }
        return pending[draft] != nil
    }

    mutating func enqueue(draft: String, account: WorkLifetime.Ticket, attempt: WorkLifetime.Ticket, send: Send) -> Enqueued {
        let result = Self.transition(state, .enqueue(draft, account, attempt, send))
        state = result.state
        guard case .enqueued(let value) = result.effect else {
            preconditionFailure("invalid draft enqueue result")
        }
        return value
    }

    mutating func finish(_ ticket: Ticket) -> [Send]? {
        let result = Self.transition(state, .finish(ticket))
        state = result.state
        guard case .finished(let sends) = result.effect else { return nil }
        return sends
    }

    mutating func retire() { state = Self.transition(state, .retire).state }

    private static func transition(_ state: State, _ event: Event) -> (state: State, effect: Effect) {
        switch (state, event) {
        case (_, .retire):
            return (.closed, .none)
        case (.closed, .enqueue):
            return (.closed, .enqueued(.refused))
        case (.closed, .finish):
            return (.closed, .none)
        case (.open(let owner, var pending), .enqueue(let draft, let account, let attempt, let send)):
            guard account == owner, !draft.isEmpty else { return (state, .enqueued(.refused)) }
            if var current = pending[draft] {
                current.sends.append(send)
                pending[draft] = current
                return (.open(owner, pending), .enqueued(.queued(current.ticket)))
            }
            let ticket = Ticket(account: account, draft: draft, attempt: attempt)
            pending[draft] = Pending(ticket: ticket, sends: [send])
            return (.open(owner, pending), .enqueued(.started(ticket)))
        case (.open(let owner, var pending), .finish(let ticket)):
            guard ticket.account == owner, let current = pending[ticket.draft], current.ticket == ticket else {
                return (state, .none)
            }
            pending.removeValue(forKey: ticket.draft)
            return (.open(owner, pending), .finished(current.sends))
        }
    }
}
