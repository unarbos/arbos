import ArbosFFI
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
    /// 2026-11-05 12:00:00 UTC, for the cases that state their own zone.
    private let noon: Int64 = 1_793_880_000_000
    private let minute: Int64 = 60_000
    private let utc = TimeZone(secondsFromGMT: 0)!

    /// Midday of the same date in whatever zone the suite runs in. `Transcript.rows` reads
    /// the system zone, so a case that drives it anchors here and not at noon UTC: a span of
    /// a few hours from local midday cannot reach a midnight, in any zone.
    private var midday: Int64 {
        let day = Calendar.current.startOfDay(for: Date(timeIntervalSince1970: Double(noon) / 1000))
        return Int64(day.timeIntervalSince1970 * 1000) + 12 * 60 * minute
    }

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
        XCTAssertTrue(Transcript.showsDivider(previous: 0, at: noon, timeZone: utc),
                      "a chat's first message names its day")
        XCTAssertFalse(Transcript.showsDivider(previous: noon, at: noon + hour - 1, timeZone: utc),
                       "a millisecond under the hour")
        XCTAssertTrue(Transcript.showsDivider(previous: noon, at: noon + hour, timeZone: utc), "exactly an hour")
        XCTAssertTrue(Transcript.showsDivider(previous: noon, at: noon + hour + 1, timeZone: utc),
                      "a millisecond over")
        XCTAssertFalse(Transcript.showsDivider(previous: noon, at: 0, timeZone: utc),
                       "a row with no clock has no time to name")
        XCTAssertTrue(Transcript.showsDivider(previous: noon, at: noon - 2 * hour, timeZone: utc),
                      "a row that arrives with an earlier clock is still an hour away")
    }

    func testTheDayIsTheReadersDay() {
        // 23:50 and 00:04 UTC, eleven minutes either side of UTC midnight.
        let late = noon + 11 * 60 * minute + 50 * minute
        let afterMidnight = late + 14 * minute
        XCTAssertTrue(Transcript.showsDivider(previous: late, at: afterMidnight, timeZone: utc),
                      "a new day, however short the gap")
        XCTAssertFalse(Transcript.showsDivider(previous: late, at: afterMidnight,
                                               timeZone: TimeZone(secondsFromGMT: -5 * 3600)!),
                       "New York reads the same two stamps as one evening")
    }

    /// The symptom in Jacob's screenshot: a chat ticking along a few minutes at a time used
    /// to collect a line every hour, mid-conversation. Now it opens with one and stops.
    func testAChatWhoseMessagesAreMinutesApartDrawsOneLineAtItsStart() {
        let messages = (0..<40).map { person("m\($0)", "steady", at: midday + Int64($0) * 6 * minute) }
        XCTAssertEqual(dateLines(messages), ["m0"], "four hours of talking, one line — the chat's first message")
    }

    func testAnHourOfSilenceDrawsALineAndTheRestOfTheRunDoesNot() {
        let hour = timeDividerGapMs()
        let messages = [
            person("m1", "morning", at: midday),
            person("m2", "still here", at: midday + 20 * minute),
            person("m3", "back", at: midday + 20 * minute + hour),
            person("m4", "and one more", at: midday + 25 * minute + hour),
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
            person("m1", "with a clock", at: midday),
            person("m2", "without one", at: 0),
            person("m3", "with a clock again", at: midday + 12 * minute),
        ]
        XCTAssertEqual(dateLines(messages), ["m1"])
    }

    /// #2047 asked for "Today" only above the first message of the day. Jacob's later rule
    /// ([feedback time-dividers-2026-11-05] / #2794) keeps an hour-gap stamp mid-day too, and
    /// its words still say "Today …". So a second same-day stamp after an hour of silence is
    /// intentional — not a placement bug.
    func testASameDayHourGapStillDrawsTodayAboveTheLaterMessage() {
        let hour = timeDividerGapMs()
        let first = midday
        let later = midday + hour
        let messages = [
            person("m1", "earlier today", at: first),
            person("m2", "after an hour", at: later),
        ]
        XCTAssertEqual(dateLines(messages), ["m1", "m2"])
        let stamp = Transcript.dateLabel(later)
        XCTAssertTrue(stamp.hasPrefix("Today ") || stamp.contains(":"),
                      "the hour-gap stamp still names the later message's clock: \(stamp)")
    }

    /// Jacob's TestFlight shot ([feedback AGgL7fykRs0BE9uiHscNCvk]): "Today" sat on
    /// the bubble above it. The shared `systemGap` is equal above and below the stamp,
    /// but a bubble's own vertical pad opens the space below before its text — so the
    /// stamp needs its own top pad, and that pad is what this pins on the view.
    /// Landed on main via #2982; kept here so this branch stays compatible after the merge.
    func testTheDateStampKeepsAirAboveTheBubbleAboveIt() throws {
        XCTAssertEqual(Transcript.stampTopPad, 16,
                       "enough air that \"Today …\" clears the bubble above it")
        XCTAssertGreaterThan(Transcript.stampTopPad, 0)
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Arbos/UI/Transcript.swift")
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw XCTSkip("sources not beside the test bundle (CI without the tree)")
        }
        let src = try String(contentsOf: url, encoding: .utf8)
        XCTAssertTrue(src.contains(".padding(.top, Transcript.stampTopPad)"),
                      "the centred system stamp draws the pad above its glyphs")
    }

    /// A stamp from another year carries the year; an old relative stamp becomes a date.
    func testOldStampsSayWhichYearAndStopCountingMonths() {
        let lastYear = Calendar.current.date(byAdding: .year, value: -1, to: Date())!
        let ms = Int64(lastYear.timeIntervalSince1970 * 1000)
        XCTAssertEqual(HomeView.timeLabel(ms: ms), lastYear.formatted(date: .numeric, time: .omitted))
        XCTAssertEqual(SecretsText.ago(ms), SecretsText.shortDate(ms))
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        XCTAssertEqual(SecretsText.ago(now), "just now")
        XCTAssertNotEqual(SecretsText.ago(now - 3 * 86_400_000), SecretsText.shortDate(now - 3 * 86_400_000))
    }
}
