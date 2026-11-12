// Grok Bot profile / edit chrome (Jacob, 2026-11-10): the overview landing
// (avatar · name · Edit · grouped rows) and the Edit / Create page (avatar with
// pencil · section headers · grouped cards · white Done/Create pill). Shared by
// agent, group, server and the create flows so every screen keeps one hierarchy
// ([feedback iphone-entity-profile-edit-2026-11-10],
// [feedback iphone-create-entity-edit-style-2026-11-10]).

import SwiftUI

// MARK: - Pills

/// The small dark "Edit" pill under a profile name (Grok Bot profile).
struct EditPillButton: View {
    @Environment(\.theme) private var theme
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text("Edit")
                .font(theme.font(Theme.small, weight: .semibold))
                .foregroundStyle(theme.text)
                .padding(.horizontal, 14)
                .frame(height: 28)
                .background(Capsule().fill(theme.field)) // control-size: exception inlineChips
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("profile-edit")
        .accessibilityLabel("Edit")
    }
}

/// The white "Done" / "Create" / "Next" pill at the top-right of an Edit or Create
/// page (Grok Bot Edit Bot). Light fill, dark label — opposite of the dark Edit pill.
struct DonePillButton: View {
    @Environment(\.theme) private var theme
    let title: String
    var enabled: Bool = true
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(theme.font(Theme.small, weight: .semibold))
                .foregroundStyle(enabled ? Color.black : theme.placeholder)
                .padding(.horizontal, 14)
                .frame(height: 28)
                .background(Capsule().fill(enabled ? Color.white : theme.field))
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        // Collapse the label into one element so XCTest finds `edit-done` on the
        // button itself (plain + disabled otherwise left the identifier stranded).
        .accessibilityElement(children: .ignore)
        .accessibilityAddTraits(.isButton)
        .accessibilityIdentifier("edit-done")
        .accessibilityLabel(title)
    }
}

/// Grok's up/down picker chevron on a value row (Voice / Speed / Language).
struct PickerGlyph: View {
    @Environment(\.theme) private var theme
    var body: some View {
        Image(systemName: "chevron.up.chevron.down")
            .font(theme.font(11, weight: .semibold))
            .foregroundStyle(theme.placeholder)
            .accessibilityHidden(true)
    }
}

// MARK: - Edit / Create page scaffold

/// Top chrome for an Edit or Create push: glass back on the left, centred title,
/// white Done/Create pill on the right. Carries `backSwipeToPop` so the edge
/// swipe pops one level ([feedback iphone-entity-nav-push-2026-11-10]).
struct EntityEditTopBar: View {
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let title: String
    var doneTitle: String = "Done"
    var doneEnabled: Bool = true
    /// nil hides the Done pill (read-only).
    var onDone: (() -> Void)? = nil
    /// Override when dismiss should clear a whole compose stack.
    var onBack: (() -> Void)? = nil

    var body: some View {
        // Title is a ZStack sibling (not between the HStack Spacers): a long
        // "Create New Agent" title was compressing the Done pill, and an HStack
        // overlay of the title collapsed controls out of the accessibility tree
        // the same way New Message's title overlay hid `new-message-back`.
        ZStack(alignment: .top) {
            HStack(spacing: 12) {
                Button { (onBack ?? { dismiss() })() } label: {
                    GlassCircle {
                        Image(systemName: "chevron.left")
                            .font(.system(size: 17, weight: .semibold))
                            .foregroundStyle(theme.text)
                    }
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Back")
                Spacer(minLength: 0)
                if let onDone {
                    DonePillButton(title: doneTitle, enabled: doneEnabled, action: onDone)
                } else {
                    Color.clear.frame(width: 56, height: 28)
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 16)
            .padding(.bottom, 8)

            Text(title)
                .font(theme.font(Theme.body, weight: .semibold))
                .foregroundStyle(theme.text)
                .lineLimit(1)
                .padding(.horizontal, 72)
                .padding(.top, 16)
                .padding(.bottom, 8)
                .frame(maxWidth: .infinity)
                .allowsHitTesting(false)
                .accessibilityAddTraits(.isHeader)
        }
        .background(theme.bg)
    }
}

/// The Edit / Create hero: large centred avatar with an optional pencil badge.
/// A tap on the pencil (or the whole picture when `onAvatar` is set) opens the
/// avatar editor — a further push, never a sheet.
struct EntityEditHero<Avatar: View>: View {
    @Environment(\.theme) private var theme
    var onAvatar: (() -> Void)? = nil
    @ViewBuilder var avatar: () -> Avatar

    var body: some View {
        Button {
            onAvatar?()
        } label: {
            avatar()
                .frame(width: PaneKit.heroAvatar, height: PaneKit.heroAvatar)
                .paneCrayon(onAvatar != nil)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .disabled(onAvatar == nil)
        .frame(maxWidth: .infinity)
        .accessibilityLabel(onAvatar == nil ? "Avatar" : "Change avatar")
        .accessibilityIdentifier("edit-avatar")
    }
}

/// Small caps section header over a card on an Edit / Create page ("Name",
/// "Appearance", "Voice Calls").
struct EntitySectionHeader: View {
    @Environment(\.theme) private var theme
    let title: String
    var body: some View {
        Text(title.uppercased())
            .font(theme.font(Theme.tiny, weight: .medium))
            .foregroundStyle(theme.muted)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, PaneKit.inset)
            .accessibilityAddTraits(.isHeader)
    }
}

/// A text field that lives inside a grouped card row (the Edit Bot Name field).
struct EntityNameField: View {
    @Environment(\.theme) private var theme
    @Binding var text: String
    var placeholder: String
    var focused: FocusState<Bool>.Binding
    var submitLabel: SubmitLabel = .done
    var onSubmit: () -> Void = {}

    var body: some View {
        TextField("", text: $text, prompt: Text(placeholder).foregroundStyle(theme.placeholder))
            .font(theme.font(Theme.body))
            .foregroundStyle(theme.text)
            .autocorrectionDisabled()
            .textInputAutocapitalization(.words)
            .focused(focused)
            .submitLabel(submitLabel)
            .onSubmit(onSubmit)
            .padding(.horizontal, PaneKit.inset)
            .frame(minHeight: PaneKit.rowHeight)
            .accessibilityIdentifier("entity-name-field")
    }
}

/// A settings value row with the Grok up/down picker glyph instead of a chevron
/// (Voice / Speed / Language on Edit Bot; Model / Reasoning / Voice here).
struct EntityPickerRow: View {
    @Environment(\.theme) private var theme
    let title: String
    var value: String? = nil
    var body: some View {
        PaneRow(title: title, value: value, chevron: false, trailing: AnyView(PickerGlyph()))
    }
}

// MARK: - Overview landing

/// Profile overview without a tab pill: hero (avatar · name · optional Edit) then
/// the grouped cards. Former tabs become rows that push further right.
struct ProfileOverview: View {
    @Environment(\.theme) private var theme
    let hero: PaneHero
    let sections: [PaneSection]
    var footer: [PaneSection] = []

    var body: some View {
        // Zero tabs → ProfilePane draws no pill ([feedback iphone-entity-profile-edit-2026-11-10]).
        ProfilePane(hero: hero, tabs: [], selected: .constant(""), body: sections, footer: footer)
    }
}

/// The overview hero's Edit pill, parked in `PaneHero.action` so it sits under
/// the name the way Grok's does (and the person's Message pill already does).
enum OverviewEdit {
    static func action(_ onEdit: @escaping () -> Void) -> AnyView {
        AnyView(EditPillButton(action: onEdit).padding(.top, PaneKit.heroActionGap))
    }
}
