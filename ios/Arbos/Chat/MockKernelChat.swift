import Foundation

/// A scripted main chat for when no kernel is reachable. Same `ChatUpdate`
/// stream as the live source, so the screen cannot tell them apart. Replies
/// stream one word at a time.
@MainActor
final class MockKernelChat: ChatSource {
    private var stream: AsyncStream<ChatUpdate>.Continuation?
    private var reply: Task<Void, Never>?

    let updates: AsyncStream<ChatUpdate>

    init() {
        var held: AsyncStream<ChatUpdate>.Continuation!
        updates = AsyncStream { held = $0 }
        stream = held
    }

    func start() async throws {
        stream?.yield(.agents([
            KernelAgent(id: "root", name: "main", parent: nil, paused: false, model: "claude-fable-5.1"),
            KernelAgent(id: "a7k2", name: "reconnect test", parent: "root", paused: false, model: ""),
        ]))
        stream?.yield(.history([
            ChatItem(.user("Where are we on the kernel branch?")),
            ChatItem(.tool(label: "grep · attach", failed: false, seconds: 1)),
            ChatItem(.tool(label: "read · serve.rs", failed: false, seconds: 0)),
            ChatItem(.agent(
                "The attach loop is in. Two things left: the reconnect test and the changelog entry.",
                streaming: false
            )),
            ChatItem(.user("Start on the test. I'll take the changelog.")),
            ChatItem(.subagent(name: "reconnect test", status: "spawned")),
            ChatItem(.agent("On it. A child is writing the reconnect test; I'll say when it is green.", streaming: false)),
            ChatItem(.subagent(name: "reconnect test", status: "three cases pass, one flaky on slow tunnels")),
            ChatItem(.tool(label: "bash · cargo test -p arbos-kernel attach", failed: false, seconds: 41)),
        ]))
        stream?.yield(.turn(running: false))
    }

    func send(text: String, steer: Bool) async throws {
        reply?.cancel()
        stream?.yield(.item(ChatItem(.user(text))))
        stream?.yield(.turn(running: true))
        let answer = Self.answer(to: text)
        reply = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(900))
            guard let self, !Task.isCancelled else { return }
            self.stream?.yield(.item(ChatItem(.tool(label: "grep · " + Self.keyword(in: text), failed: false, seconds: 1))))
            try? await Task.sleep(for: .milliseconds(500))
            for word in answer.split(separator: " ", omittingEmptySubsequences: false) {
                guard !Task.isCancelled else { return }
                self.stream?.yield(.agentDelta(String(word) + " "))
                try? await Task.sleep(for: .milliseconds(70))
            }
            self.stream?.yield(.agentDone)
            self.stream?.yield(.turn(running: false))
        }
    }

    func stop() {
        reply?.cancel()
        stream?.finish()
    }

    private static func keyword(in text: String) -> String {
        text.split(separator: " ")
            .map(String.init)
            .filter { $0.count > 4 }
            .first?
            .lowercased()
            .trimmingCharacters(in: .punctuationCharacters) ?? "kernel"
    }

    private static func answer(to text: String) -> String {
        let lower = text.lowercased()
        if lower.contains("merge") || lower.contains("left") {
            return "Two things before merge. The reconnect test still fails once in five runs on a slow tunnel; "
                + "I have the fix, a longer settle window in wait_alive. And the changelog entry is yours. "
                + "Say go and I land the fix and open the PR."
        }
        if lower.contains("status") || lower.contains("where") {
            return "Attach loop done, reconnect test mostly green, changelog pending. Nothing blocked on you."
        }
        return "Got it. I will do that now and say when it is done."
    }
}
