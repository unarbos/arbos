import XCTest

/// Pays the first app launch of the UI run so that no real case does. A mitigation: the cause
/// is not known.
///
/// The first launch of a run can stall inside xcodebuild on the host. In run 41795222240 its main
/// thread went silent for 76.5 s right after it took the launch request, longer than XCUITest's
/// fixed 60 s "requesting launch progress" wait, and the first case failed before the app was even
/// installed. Every later launch in that run was fine.
///
/// The class must sort first: UI cases run in case-insensitive alphabetical order, class first.
/// An `-only-testing` filter that leaves it out leaves the first real case to pay the stall.
///
/// Issues are dropped in `record(_:)`, not matched with `XCTExpectFailure`: a failed launch
/// interrupts the case even when its failure is expected, so the `terminate()` below would never
/// run, and the app that xcodebuild still launches once the stall clears would be left running for
/// the next case.
final class AAAWarmUpUITests: XCTestCase {
    override func record(_ issue: XCTIssue) {
        print("warm-up launch issue, discarded: \(issue.compactDescription)")
    }

    func testTheFirstLaunchOfTheRunIsDiscarded() {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1"]
        app.launch()
        if app.state != .runningForeground {
            app.launch()
        }
        app.terminate()
    }
}
