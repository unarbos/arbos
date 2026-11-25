// Settings → Bot → Timezone: which zone the agents answer in.
//
// The zone travels to the agent on every message this phone sends, as the row's `tz` field
// (core `UserMessageOpts`), which agentd turns into the turn's "Now" line. By default it is
// whatever iOS reports; this screen lets one account pin a different zone, and that choice then
// stands in for the device's everywhere the device's would be used.
//
// Kept per account in `UserDefaults` under `timezone`, a map of account id to IANA zone name.
// Per account because two people signing into the same phone must not inherit each other's
// zone, and an account with no entry means auto-detect, not "no zone".

import SwiftUI

enum TimeZoneSetting {
    /// The `UserDefaults` key holding the account-to-zone map.
    private static let key = "timezone"

    /// The signed-in account, kept current by `AppModel.user`. Empty while nobody is signed in,
    /// which is also the only state in which a zone cannot be pinned.
    static var account: String = ""

    /// What iOS says this phone's zone is.
    static var deviceZone: String { TimeZone.current.identifier }

    /// Every zone the phone can be pinned to, in alphabetical order.
    static var allZones: [String] { TimeZone.knownTimeZoneIdentifiers.sorted() }

    /// The zone this account pinned by hand, if any. A name iOS no longer knows is no pin at
    /// all, so a zone dropped from a later iOS release falls back to auto-detect rather than
    /// pinning the agent to a name nothing can parse.
    static func chosen(account: String = TimeZoneSetting.account) -> String? {
        guard !account.isEmpty else { return nil }
        let map = UserDefaults.standard.dictionary(forKey: key) as? [String: String] ?? [:]
        guard let zone = map[account]?.trimmingCharacters(in: .whitespaces), known(zone) else { return nil }
        return zone
    }

    /// Pin `zone` for this account, or clear the pin back to auto-detect with `nil`.
    static func choose(_ zone: String?, account: String = TimeZoneSetting.account) {
        guard !account.isEmpty else { return }
        var map = UserDefaults.standard.dictionary(forKey: key) as? [String: String] ?? [:]
        if let zone, known(zone) {
            map[account] = zone
        } else {
            map.removeValue(forKey: account)
        }
        UserDefaults.standard.set(map, forKey: key)
    }

    /// The zone the agents are told about: this account's pin when it has one, else the
    /// phone's own. Read on every message send, so it must stay cheap.
    static func effective(account: String = TimeZoneSetting.account) -> String {
        chosen(account: account) ?? deviceZone
    }

    /// What the Timezone row shows on its right: the zone the agents are actually told about,
    /// pinned or detected. Only the zone — `Auto-detect (America/Costa_Rica)` is too wide to
    /// sit beside the label on the narrow Mac pane, and the two platforms say the same thing.
    /// The footnote under the card carries which of the two states it is.
    static func rowValue(account: String = TimeZoneSetting.account) -> String {
        let zone = effective(account: account)
        return zone.isEmpty ? "Not set" : zone
    }

    /// The line under the card: whether this zone follows the phone or was pinned by hand.
    static func footnote(account: String = TimeZoneSetting.account) -> String {
        chosen(account: account) == nil
            ? "Agents answer times, dates and \u{201C}today\u{201D} in this zone, detected from this iPhone."
            : "Agents answer times, dates and \u{201C}today\u{201D} in this zone. It stays set when you travel."
    }

    /// How the zone list names the auto-detect entry, where there is room for both facts.
    static func autoLabel(device: String = TimeZoneSetting.deviceZone) -> String {
        device.isEmpty ? "Auto-detect" : "Auto-detect (\(device))"
    }

    /// The zones a query matches, case-insensitively, on the zone name with `_` read as a space
    /// so "costa rica" finds `America/Costa_Rica`. An empty query matches everything.
    static func matching(_ query: String, in zones: [String] = TimeZoneSetting.allZones) -> [String] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
            .replacingOccurrences(of: "_", with: " ")
        guard !q.isEmpty else { return zones }
        return zones.filter { $0.lowercased().replacingOccurrences(of: "_", with: " ").contains(q) }
    }

    private static func known(_ zone: String) -> Bool {
        !zone.isEmpty && TimeZone(identifier: zone) != nil
    }
}

/// The zone list, built as the Model picker is (`ModelPickerView`): a filter field, then every
/// zone with a checkmark on the current one. `List` keeps it lazy — there are about six hundred
/// zones, and a card of eagerly built rows would hitch the sheet as it rises.
///
/// Auto-detect is the first row, so the way back to the phone's own zone is never more than a
/// scroll to the top. A tap saves and closes.
struct TimezonePickerView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    /// Bumped on each pick so the checkmark follows: `UserDefaults` is not observable, and a
    /// published mirror of it would be a second source of truth for one row.
    @State private var saved = 0

    private var account: String { model.user?.id ?? "" }

    private var chosen: String? {
        _ = saved
        return TimeZoneSetting.chosen(account: account)
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    HStack(spacing: 8) {
                        Image(systemName: "magnifyingglass")
                            .font(theme.font(15, weight: .medium))
                            .foregroundStyle(theme.muted)
                        TextField("", text: $query, prompt: Text("Filter time zones").foregroundStyle(theme.placeholder))
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .font(theme.font(Theme.body))
                            .foregroundStyle(theme.text)
                        if !query.isEmpty {
                            Button { query = "" } label: {
                                Image(systemName: "xmark.circle.fill").foregroundStyle(theme.muted)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .listRowBackground(theme.card)
                }
                Section {
                    row(title: TimeZoneSetting.autoLabel(), selected: chosen == nil) { pick(nil) }
                } footer: {
                    Text(TimeZoneSetting.footnote(account: account))
                        .font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                }
                Section {
                    let matches = TimeZoneSetting.matching(query)
                    if matches.isEmpty {
                        Text("No time zone matches “\(query.trimmingCharacters(in: .whitespaces))”.")
                            .foregroundStyle(theme.muted)
                            .listRowBackground(theme.card)
                    }
                    ForEach(matches, id: \.self) { zone in
                        row(title: zone, selected: chosen == zone) { pick(zone) }
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(theme.bg)
            .navigationTitle("Timezone")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
    }

    @ViewBuilder
    private func row(title: String, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack {
                Text(title).foregroundStyle(theme.text)
                Spacer()
                if selected {
                    Image(systemName: "checkmark").foregroundStyle(theme.text)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowBackground(theme.card)
        .accessibilityLabel("\(title)\(selected ? ", selected" : "")")
        .accessibilityAddTraits(selected ? [.isSelected] : [])
    }

    private func pick(_ zone: String?) {
        TimeZoneSetting.choose(zone, account: account)
        saved += 1
        dismiss()
    }
}
