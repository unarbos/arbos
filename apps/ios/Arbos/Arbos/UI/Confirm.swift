// The one confirm ([feedback confirm-alerts-ios-2026-10-27], Jacob 2026-10-27): a question
// before a consequential action is a centred alert — title, a one-line note, Cancel · action —
// the Mac's two-button confirm. On iOS 26 a `.confirmationDialog` presents anchored to the view
// it is attached to; attached at a screen's root (as every one of ours was) it floated in the
// middle of the transcript over someone's messages, with no Cancel. `.confirmationDialog` is
// kept only where it sits on its own control (a row's menu); `ConfirmFormLintTests` holds the
// line: every other `.confirmationDialog(` outside this file fails the build's tests.

import SwiftUI

/// What a confirm says: the alert's title, its note, the action button and Cancel.
/// Plain data so tests pin the shape without rendering.
struct ConfirmSpec: Equatable {
    var title: String
    var message: String?
    var action: String
    /// Destructive confirms draw the action in red (every "Delete", "Leave", "Block").
    var destructive: Bool = true
    var cancel: String = "Cancel"

    /// Button order as the alert lays them out: the action, then Cancel.
    var buttons: [String] { [action, cancel] }
}

extension View {
    /// Ask before `perform`: a centred alert with `title`, `message`, Cancel and the
    /// `action` button. `destructive` colours the action red (the default — most confirms
    /// guard a removal); pass `false` for a plain one (Unblock, Sign out of a device you
    /// can sign back into).
    func confirm(_ spec: ConfirmSpec, isPresented: Binding<Bool>,
                 onCancel: @escaping () -> Void = {}, perform: @escaping () -> Void) -> some View {
        alert(spec.title, isPresented: isPresented) {
            Button(spec.action, role: spec.destructive ? ButtonRole.destructive : nil, action: perform)
            Button(spec.cancel, role: .cancel, action: onCancel)
        } message: {
            if let message = spec.message { Text(message) }
        }
    }

    /// The same for a confirm keyed on an item ("Remove @sam?"): shown while `item` is set,
    /// cleared on either button. `spec` builds the words from the item.
    func confirm<T>(item: Binding<T?>, spec: @escaping (T) -> ConfirmSpec,
                    perform: @escaping (T) -> Void) -> some View {
        let current = item.wrappedValue
        let shown = Binding(get: { item.wrappedValue != nil }, set: { if !$0 { item.wrappedValue = nil } })
        return confirm(current.map(spec) ?? ConfirmSpec(title: "", action: ""), isPresented: shown,
                       onCancel: { item.wrappedValue = nil }) {
            if let value = item.wrappedValue { perform(value) }
            item.wrappedValue = nil
        }
    }
}
