import Foundation

/// Local acceptance authority for asynchronous work. Closing an owner invalidates
/// its tickets before resource cleanup awaits; reopening never revives an old ticket.
struct WorkLifetime {
    struct Ticket: Hashable, Sendable {
        fileprivate let owner: UUID
        fileprivate let generation: UInt64
    }

    enum State: Equatable {
        case accepting(UInt64)
        case closed(UInt64)
    }

    enum Event { case reopen, close }

    private let owner = UUID()
    private(set) var state: State = .accepting(0)

    static func transition(_ state: State, _ event: Event) -> State {
        let generation: UInt64
        switch state {
        case .accepting(let current), .closed(let current): generation = current
        }
        precondition(generation < UInt64.max, "work lifetime exhausted")
        switch event {
        case .reopen: return .accepting(generation + 1)
        case .close: return .closed(generation + 1)
        }
    }

    var ticket: Ticket? {
        guard case .accepting(let generation) = state else { return nil }
        return Ticket(owner: owner, generation: generation)
    }

    func accepts(_ ticket: Ticket) -> Bool { self.ticket == ticket }

    @discardableResult
    mutating func reopen() -> Ticket {
        state = Self.transition(state, .reopen)
        return ticket!
    }

    mutating func close() { state = Self.transition(state, .close) }
}

/// Startup and callbacks belong to the same Matrix client attempt and account.
struct MatrixLifetime {
    struct Ticket: Equatable, Sendable {
        fileprivate let owner: UUID
        fileprivate let generation: UInt64
        let account: WorkLifetime.Ticket
    }

    enum State: Equatable {
        case idle(UInt64)
        case starting(Ticket)
        case running(Ticket)
    }

    enum Event {
        case start(Ticket)
        case ready(Ticket)
        case failed(Ticket)
        case stop
    }

    private let owner = UUID()
    private(set) var state: State = .idle(0)

    private var generation: UInt64 {
        switch state {
        case .idle(let generation): return generation
        case .starting(let ticket), .running(let ticket): return ticket.generation
        }
    }

    static func transition(_ state: State, _ event: Event) -> State {
        switch (state, event) {
        case (.idle, .start(let ticket)): return .starting(ticket)
        case (.starting(let current), .ready(let ticket)) where current == ticket:
            return .running(ticket)
        case (.starting(let current), .failed(let ticket)) where current == ticket:
            return .idle(current.generation)
        case (_, .stop):
            let generation: UInt64
            switch state {
            case .idle(let n): generation = n
            case .starting(let ticket), .running(let ticket): generation = ticket.generation
            }
            precondition(generation < UInt64.max, "matrix lifetime exhausted")
            return .idle(generation + 1)
        default: return state
        }
    }

    mutating func start(account: WorkLifetime.Ticket) -> Ticket? {
        guard case .idle = state else { return nil }
        precondition(generation < UInt64.max, "matrix lifetime exhausted")
        let ticket = Ticket(owner: owner, generation: generation + 1, account: account)
        state = Self.transition(state, .start(ticket))
        return ticket
    }

    func accepts(_ ticket: Ticket) -> Bool {
        switch state {
        case .starting(let current), .running(let current): return current == ticket
        case .idle: return false
        }
    }

    mutating func ready(_ ticket: Ticket) { state = Self.transition(state, .ready(ticket)) }
    mutating func failed(_ ticket: Ticket) { state = Self.transition(state, .failed(ticket)) }
    mutating func stop() { state = Self.transition(state, .stop) }
}

struct SnapshotPublication {
    struct Ticket: Sendable {
        let account: WorkLifetime.Ticket
        let doc: ObjectIdentifier
        let revision: Int
    }

    enum Target {
        case disposed
        case active(doc: ObjectIdentifier, revision: Int)
    }

    static func accepts(_ ticket: Ticket, account: WorkLifetime, target: Target, empty: Bool) -> Bool {
        guard account.accepts(ticket.account), case .active(let doc, let revision) = target,
              ticket.doc == doc else { return false }
        // Keep the existing first-paint escape only within this live doc's lifetime.
        return ticket.revision == revision || empty
    }
}
