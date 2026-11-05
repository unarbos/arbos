// The Search-in-chat field, as a `UITextField` rather than SwiftUI's `TextField`.
//
// One reason, and it is the whole reason: ⌫ with the caret in an empty field has
// to reach the bar, so the bar can take the pinned sender off the way the
// composer's strip drops its last staged attachment ([feedback
// composer-attachment-removal-ios-2026-10-27]) and the way the Mac's chip has
// since `f3632d286`. SwiftUI offers no hook for it — `.onKeyPress` sees a
// hardware keyboard only, and the software keyboard's backspace on an empty
// field raises no key event at all, so the one gesture that matters here is the
// one SwiftUI cannot report.
//
// The composer answered the same problem the same way (`MentionTextView`, #1596),
// and this follows it deliberately: an `isFocused` binding that the first
// responder follows both ways, a `didMoveToWindow` hook so focus asked for before
// the field is in a window still lands, and the theme applied only on a change.
// It is the plain single-line field it replaces in every other respect.

import SwiftUI
import UIKit

struct ChatSearchField: UIViewRepresentable {
    @Binding var text: String
    /// The first responder follows this both ways, like the composer's field.
    @Binding var isFocused: Bool
    let placeholder: String
    let theme: Theme
    /// ⌫ with nothing to delete: the owner answers whether it took the key.
    /// `false` — nothing pinned — leaves it the field's own, where it deletes
    /// nothing, so the bar never swallows a key it did not act on.
    var onDeleteBackwardEmpty: (() -> Bool)? = nil

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> ChatSearchUITextField {
        let view = ChatSearchUITextField()
        let coordinator = context.coordinator
        view.delegate = coordinator
        view.backgroundColor = .clear
        view.borderStyle = .none
        view.clearButtonMode = .never
        view.autocorrectionType = .no
        view.autocapitalizationType = .none
        view.spellCheckingType = .no
        view.smartQuotesType = .no
        view.smartDashesType = .no
        view.returnKeyType = .search
        view.enablesReturnKeyAutomatically = false
        view.adjustsFontForContentSizeCategory = false
        // The identifier the existing bar carried, so the rig and the UI tests
        // keep finding the field.
        view.accessibilityIdentifier = "chat-search-field"
        view.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        view.setContentHuggingPriority(.defaultLow, for: .horizontal)
        view.addTarget(coordinator, action: #selector(Coordinator.editingChanged(_:)), for: .editingChanged)
        view.onDeleteBackwardEmpty = onDeleteBackwardEmpty
        view.onMoveToWindow = { [weak view, weak coordinator] in
            guard let view, let coordinator, view.window != nil else { return }
            coordinator.syncFocus(view)
        }
        coordinator.apply(theme: theme, placeholder: placeholder, to: view)
        if view.text != text { view.text = text }
        return view
    }

    func updateUIView(_ view: ChatSearchUITextField, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self
        view.onDeleteBackwardEmpty = onDeleteBackwardEmpty
        coordinator.apply(theme: theme, placeholder: placeholder, to: view)
        // Text changed from outside (the bar cleared it on close, the rig typed
        // into it): take it, caret at the end, which is where a cleared field
        // puts it anyway.
        if view.text != text { view.text = text }
        coordinator.syncFocus(view)
    }

    @MainActor
    final class Coordinator: NSObject, UITextFieldDelegate {
        var parent: ChatSearchField
        private var themeKey = ""
        private var placeholderKey = ""

        init(_ parent: ChatSearchField) {
            self.parent = parent
        }

        @objc func editingChanged(_ field: UITextField) {
            let typed = field.text ?? ""
            if parent.text != typed { parent.text = typed }
        }

        func textFieldDidBeginEditing(_ field: UITextField) {
            if !parent.isFocused { parent.isFocused = true }
        }

        func textFieldDidEndEditing(_ field: UITextField) {
            if parent.isFocused { parent.isFocused = false }
        }

        /// Return / Search closes the keyboard and leaves the results up.
        func textFieldShouldReturn(_ field: UITextField) -> Bool {
            field.resignFirstResponder()
            return false
        }

        /// First responder follows `isFocused`, both ways (see the delegate).
        func syncFocus(_ view: UITextField) {
            guard view.window != nil else { return }
            if parent.isFocused, !view.isFirstResponder {
                DispatchQueue.main.async { view.becomeFirstResponder() }
            } else if !parent.isFocused, view.isFirstResponder {
                DispatchQueue.main.async { view.resignFirstResponder() }
            }
        }

        /// Only on a change: setting these restyles the field and would disturb
        /// an in-progress IME composition.
        func apply(theme: Theme, placeholder: String, to view: UITextField) {
            let key = "\(theme.isDark)|\(theme.textScale)"
            let font = UIFont.systemFont(ofSize: theme.pt(Theme.body))
            if view.font != font { view.font = font }
            let text = UIColor(theme.text)
            if view.textColor != text { view.textColor = text }
            let tint = UIColor(theme.accent)
            if view.tintColor != tint { view.tintColor = tint }
            if key != themeKey || placeholder != placeholderKey {
                themeKey = key
                placeholderKey = placeholder
                view.attributedPlaceholder = NSAttributedString(
                    string: placeholder,
                    attributes: [.font: font, .foregroundColor: UIColor(theme.placeholder)])
            }
        }
    }
}

/// The field itself. Everything here is the one override that SwiftUI cannot
/// give: a ⌫ the field has nothing to delete for goes to its owner.
final class ChatSearchUITextField: UITextField {
    /// Focus asked for before the field was in a window (⌘F and the profile
    /// tile both do that) lands when it arrives.
    var onMoveToWindow: (() -> Void)?
    var onDeleteBackwardEmpty: (() -> Bool)?

    override func didMoveToWindow() {
        super.didMoveToWindow()
        onMoveToWindow?()
    }

    /// `MentionUITextView.deleteBackward`'s rule, on this app's other field:
    /// with text to delete the key is the field's, so a keyword is never traded
    /// for the chip; with the field empty the owner takes the key, removes the
    /// chip and does not close the bar.
    override func deleteBackward() {
        if (text ?? "").isEmpty, onDeleteBackwardEmpty?() == true { return }
        super.deleteBackward()
    }
}
