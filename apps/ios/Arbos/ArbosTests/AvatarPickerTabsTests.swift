import XCTest
@testable import Arbos

/// The avatar picker offers the same four controls to every target (Jacob, 2026-10-24: the
/// group / server picker had only Bot / Upload / Reset).
final class AvatarPickerTabsTests: XCTestCase {
    func testEveryTargetGetsBotGenerateUploadAndReset() {
        for target in [AvatarTarget.agent("a"), .group("g"), .server("s")] {
            XCTAssertEqual(AvatarPickerSheet.tabs(for: target), [.bot, .generate, .upload], "\(target)")
        }
        XCTAssertEqual(AvatarPickerTab.allCases.count, 3)
    }
}
