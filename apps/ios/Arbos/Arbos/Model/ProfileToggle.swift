import Foundation

// One behaviour for every picture or name of a person or an agent in a chat
// ([feedback profile-pane-toggle-ios-2026-10-26]; Jacob 2026-10-26: "clicking the
// agent or user in the chat should open their user profile … Clicking it again
// should close it"). The Mac's `ui::profile_target` twin: a sender name, the
// avatar beside a bubble, an @mention chip, the header pill and a status-row
// picture all go through `AppModel.toggleProfile`, which asks `next` — open the
// sheet, dismiss it when that same person or agent is already up (whichever
// target opened it), swap it for someone else — and read `label` for VoiceOver.

enum ProfileToggle {
    /// What a tap on `tapped` does while `showing` is up: `nil` dismisses the
    /// sheet (the same subject again), anything else is the page to present.
    static func next(showing: ProfileTarget?, tapped: ProfileTarget) -> ProfileTarget? {
        if let showing, showing.sameSubject(as: tapped) { return nil }
        return tapped
    }

    /// The accessible label of a target: what one tap will do.
    static func label(name: String, showing: Bool) -> String {
        showing ? "Close profile" : "Open \(name.trimmingCharacters(in: .whitespaces))'s profile"
    }
}

extension ProfileTarget {
    /// The same person, agent, group, server or channel. A person is the same
    /// person by id or by handle (case-insensitive) — a chip's `ProfilePerson`
    /// and a member row's can carry different ids for one account.
    func sameSubject(as other: ProfileTarget) -> Bool {
        switch (self, other) {
        case let (.user(a), .user(b)):
            return a.id == b.id || a.handle.caseInsensitiveCompare(b.handle) == .orderedSame
        default:
            return id == other.id
        }
    }
}
