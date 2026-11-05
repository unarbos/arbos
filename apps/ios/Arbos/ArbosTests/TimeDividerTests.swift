import XCTest
@testable import Arbos

/// The centred time line ([feedback time-dividers-2026-11-05], Jacob 2026-11-05, from his
/// own screenshot): one lands when the local day changes, or when an hour or more passed
/// since the message above it — and nowhere else. The rule is one shared function,
/// `arbos_core::transcript::shows_divider`, reached through `ArbosFFI`, so the iPhone and
/// the Mac cannot answer the question differently. Each message's own time is still on the
/// drag-left reveal (`RowView.swipeTimeLabel`), which this does not touch.
final class TimeDividerTests: XCTestCase {
    private let chat = "fx-group-launch"
    /// 2026-11-05 12:00:00 UTC. Midday, so the short gaps below cannot cross a day.
    private let noon: Int64 = 1_793_880_000_000
    private let minute: Int64 = 60_000

    private func person(_ id: String, _ text: String, at: Int64) -> Message {
        Message(id: id, chatId: chat, role: .user, sender: "user:fx-u3", status: .done, turn: nil, decision: nil,
                subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at,
                completedAt: nil, pending: false, parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0,
                senderHandle: "mira-k", senderName: "Mira Kaplan")
    }

    /// The message ids that drew a date line, in order.
    private func dateLines(_ messages: [Message]) -> [String] {
        Transcript.rows(messages, projectId: "people:\(chat)", me: "user:fx-me", people: true, multiParty: true)
            .filter { $0.id.hasSuffix("#date") }
            .map(\.messageId)
    }

    func testTheRuleIsTheSharedCoreRule() {
        let hour = timeDividerGapMs()
        XCTAssertEqual(hour, 3_600_000, "the hour is core's, not a literal on the phone")
        XCTAssertTrue(Transcript.showsDivider(previous: 0, at: noon), "a chat's first message names its day")
        XCTAssertFalse(Transcript.showsDivider(previous: noon, at: noon + hour - 1), "a millisecond under the hour")
        XCTAssertTrue(Transcript.showsDivider(previous: noon, at: noon + hour), "exactly an hour")
        XCTAssertTrue(Transcript.showsDivider(previous: noon, at: noon + hour + 1), "a millisecond over")
        XCTAssertFalse(Transcript.showsDivider(previous: noon, at: 0), "a row with no clock has no time to name")
        XCTAssertTrue(Transcript.showsDivider(previous: noon, at: noon - 2 * hour),
                      "a row that arrives with an earlier clock is still an hour away")
    }

    func testTheDayIsTheReadersDay() {
        // 23:50 and 00:04 UTC, eleven minutes either side of UTC midnight.
        let late = noon + 11 * 60 * minute + 50 * minute
        let afterMidnight = late + 14 * minute
        XCTAssertTrue(Transcript.showsDivider(previous: late, at: afterMidnight,
                                              timeZone: TimeZone(secondsFromGMT: 0)!),
                      "a new day, however short the gap")
        XCTAssertFalse(Transcript.showsDivider(previous: late, at: afterMidnight,
                                               timeZone: TimeZone(secondsFromGMT: -5 * 3600)!),
                       "New York reads the same two stamps as one evening")
    }

    /// The symptom in Jacob's screenshot: a chat ticking along a few minutes at a time used
    /// to collect a line every hour, mid-conversation. Now it opens with one and stops.
    func testAChatWhoseMessagesAreMinutesApartDrawsOneLineAtItsStart() {
        let messages = (0..<40).map { person("m\($0)", "steady", at: noon + Int64($0) * 6 * minute) }
        XCTAssertEqual(dateLines(messages), ["m0"], "four hours of talking, one line — the chat's first message")
    }

    func testAnHourOfSilenceDrawsALineAndTheRestOfTheRunDoesNot() {
        let hour = timeDividerGapMs()
        let messages = [
            person("m1", "morning", at: noon),
            person("m2", "still here", at: noon + 20 * minute),
            person("m3", "back", at: noon + 20 * minute + hour),
            person("m4", "and one more", at: noon + 25 * minute + hour),
        ]
        XCTAssertEqual(dateLines(messages), ["m1", "m3"])
    }

    /// Five minutes either side of the reader's own midnight, whatever zone this runs in:
    /// far too short for the hour, and still a day the transcript has to name.
    func testANewDayDrawsALineOnAGapTooShortForTheHour() {
        let midnight = Calendar.current.startOfDay(for: Date(timeIntervalSince1970: Double(noon) / 1000))
        let before = Int64(midnight.timeIntervalSince1970 * 1000) - 2 * minute
        let after = Int64(midnight.timeIntervalSince1970 * 1000) + 3 * minute
        let messages = [person("m1", "last of the night", at: before), person("m2", "one more thought", at: after)]
        XCTAssertEqual(dateLines(messages), ["m1", "m2"])
    }

    /// A row with no clock draws no line, and does not become the clock the next message is
    /// measured against.
    func testARowWithNoClockNeitherDrawsALineNorMovesTheOneBeforeIt() {
        let messages = [
            person("m1", "with a clock", at: noon),
            person("m2", "without one", at: 0),
            person("m3", "with a clock again", at: noon + 12 * minute),
        ]
        XCTAssertEqual(dateLines(messages), ["m1"])
    }
}
