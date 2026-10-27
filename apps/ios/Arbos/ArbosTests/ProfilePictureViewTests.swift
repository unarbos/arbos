import XCTest
@testable import Arbos

/// Jacob, 2026-10-25: a tap on a profile picture opens it full-size in the media viewer chat
/// photos use; where the tap opens the avatar picker, the picker gains View beside Reset. A
/// letter / leaf avatar with no picture has nothing to view. The Mac twin is
/// `crates/arbos-mac/src/ui/avatar_picker.rs` `me_picker_tests`.
@MainActor
final class ProfilePictureViewTests: XCTestCase {
    private func project(id: String, imageKey: String?) -> Project {
        var p = Project(
            id: id, name: "Affine", icon: "A", color: "#8b7cf6", state: .awake, stateSince: nil, stateDetail: nil,
            model: "", effort: .low, spendTodayUsd: 0, spendCapUsd: 0, avatar: nil, avatarColor: nil
        )
        // The picture is a fact (`imageKey` + stamp); its URL is the resolver's, never a copy
        // a surface holds (ARB-394, one avatar resolver).
        p.imageKey = imageKey
        p.avatarVersion = imageKey == nil ? 0 : 1_700_000_000_000
        return p
    }

    func testAProfilePictureIsOnePictureOnlyViewerPage() {
        let item = MediaAlbum.picture(name: "Sam Rivera", url: "https://relay.test/avatars/gh_sam.jpg?v=2")
        XCTAssertEqual(item.kind, .image)
        XCTAssertNil(item.blobRef, "not a chat attachment: Download / Forward stay off")
        XCTAssertEqual(item.pictureURL, "https://relay.test/avatars/gh_sam.jpg?v=2")
        XCTAssertEqual(item.by, "Sam Rivera")
        XCTAssertEqual(item.subline, "Profile picture", "the caption, not a share time")
        let state = MediaViewerState(items: [item], index: 0)
        XCTAssertEqual(state.items.count, 1)
    }

    func testThePickerOffersViewOnlyWhenThereIsAPicture() {
        let model = AppModel()
        model.projects = [
            project(id: "01PIC", imageKey: "avatar-gen-1.jpg"),
            project(id: "01LEAF", imageKey: nil),
        ]
        let pic = AvatarPickerSheet.viewablePicture(target: .agent("01PIC"), model: model)
        XCTAssertEqual(pic?.name, "Affine")
        // The public versioned picture route — the one URL every surface fetches — not the
        // owner-only blob slot.
        XCTAssertEqual(pic?.url, "\(model.relayHttpUrl)/projects/01PIC/avatar.jpg?v=1700000000000")
        XCTAssertFalse(pic?.url.contains("/blob/") ?? true)
        XCTAssertNil(AvatarPickerSheet.viewablePicture(target: .agent("01LEAF"), model: model), "a leaf has nothing to view")
        XCTAssertNil(AvatarPickerSheet.viewablePicture(target: .me, model: model), "no account, no picture")
        XCTAssertEqual(AvatarPickerSheet.tabs(for: .me), [.generate, .upload], "View lives in the bar, not as a tab")
    }
}
