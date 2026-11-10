import XCTest
@testable import Arbos

/// Jacob 2026-11-10 ([feedback solo-1to1-no-agent-reply-quote-ios-2026-11-10],
/// [feedback solo-1to1-no-agent-reply-quote-mac-2026-11-10]): in a solo 1:1 agent chat the
/// agent's bubbles never show a reply/quote header — even when an older box still attached
/// `replyTo`. Groups and channels still quote. A human's own explicit reply still quotes.
/// Mac twin: `transcript::solo_agent_reply_quote_tests`.
final class SoloAgentReplyQuoteTests: XCTestCase {
    private let chat = "fx-solo-agent"
    private let t0: Int64 = 1_700_000_000_000

    private func user(_ id: String, _ text: String, replyTo: String? = nil) -> Message {
        var m = Message(id: id, chatId: chat, role: .user, sender: "user", status: .done, turn: nil,
                        decision: nil, subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:],
                        model: nil, createdAt: t0, completedAt: nil, pending: false,
                        parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0)
        m.replyTo = replyTo
        return m
    }

    private func agent(_ id: String, _ text: String, replyTo: String? = nil) -> Message {
        var m = Message(id: id, chatId: chat, role: .assistant, sender: "agent:fx-box", status: .done,
                        turn: .reply, decision: "reply", subagentId: nil, subagentName: nil,
                        inReplyTo: replyTo, usage: [:], model: nil, createdAt: t0 + 1_000,
                        completedAt: nil, pending: false,
                        parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0)
        m.replyTo = replyTo
        return m
    }

    private func person(_ id: String, sender: String, handle: String, _ text: String) -> Message {
        Message(id: id, chatId: chat, role: .user, sender: sender, status: .done, turn: nil, decision: nil,
                subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                createdAt: t0, completedAt: nil, pending: false,
                parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0,
                senderHandle: handle, senderName: handle)
    }

    private func groupAgent(_ id: String, _ text: String, replyTo: String) -> Message {
        var m = Message(id: id, chatId: chat, role: .assistant, sender: "agent:fx-comms", status: .done,
                        turn: .reply, decision: "reply", subagentId: nil, subagentName: nil,
                        inReplyTo: replyTo, usage: [:], model: nil, createdAt: t0 + 1_000,
                        completedAt: nil, pending: false,
                        parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0,
                        senderHandle: "comms", senderName: "Comms")
        m.replyTo = replyTo
        return m
    }

    private func assistantQuote(_ rows: [Row]) -> Quote? {
        for row in rows {
            if case .assistant(_, _, _, let quote) = row.kind { return quote }
            if case .member(_, _, _, _, _, _, _, let quote) = row.kind { return quote }
        }
        return nil
    }

    private func userQuote(_ rows: [Row]) -> Quote? {
        for row in rows {
            if case .user(_, _, let quote, _) = row.kind { return quote }
        }
        return nil
    }

    func testSoloAgentChatHidesAgentReplyQuoteEvenWhenReplyToIsSet() {
        let ask = user("u1", "what's the weather")
        let answer = agent("a1", "sunny", replyTo: "u1")
        let rows = Transcript.rows([ask, answer], projectId: "fx-box", me: nil, people: false, multiParty: false)
        XCTAssertNil(assistantQuote(rows), "solo 1:1: agent bubble carries no quote header")
        XCTAssertFalse(Transcript.carriesQuote(answer, people: false, multiParty: false))
    }

    func testGroupStillShowsAgentReplyQuote() {
        let ask = person("u1", sender: "user:fx-me", handle: "jacob", "what's the weather")
        let answer = groupAgent("a1", "sunny", replyTo: "u1")
        let rows = Transcript.rows([ask, answer], projectId: "people:\(chat)", me: "user:fx-me",
                                   people: true, multiParty: true)
        let quote = assistantQuote(rows)
        XCTAssertEqual(quote?.messageId, "u1")
        XCTAssertEqual(quote?.text, "what's the weather")
        XCTAssertTrue(Transcript.carriesQuote(answer, people: true, multiParty: true))
    }

    func testHumanExplicitReplyInSoloStillShowsQuote() {
        let first = user("u1", "first thought")
        var reply = user("u2", "following up")
        reply.replyTo = "u1"
        reply.createdAt = t0 + 2_000
        let rows = Transcript.rows([first, reply], projectId: "fx-box", me: nil, people: false, multiParty: false)
        let quote = userQuote(rows)
        XCTAssertEqual(quote?.messageId, "u1")
        XCTAssertEqual(quote?.text, "first thought")
        XCTAssertTrue(Transcript.carriesQuote(reply, people: false, multiParty: false),
                      "a human's own reply still quotes in 1:1")
    }
}
