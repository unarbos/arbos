import SwiftUI

/// The first screen: every project the phone can reach, one row each,
/// the way the desktop's tabs name them — the glyph in the project's
/// colour, the name, the machine beneath. A row opens the project's chat.
struct ProjectsView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var projects: ProjectStore
    @EnvironmentObject private var chat: ChatStore
    @State private var showSettings = false

    var body: some View {
        ZStack {
            ArbosTheme.bg.ignoresSafeArea()
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(projects.entries) { entry in
                        NavigationLink(value: entry.target) {
                            ProjectRow(entry: entry, working: isWorking(entry))
                        }
                        .buttonStyle(.plain)
                    }
                    if projects.entries.isEmpty {
                        emptyState
                    }
                }
                .padding(.horizontal, ArbosTheme.gutter)
                .padding(.top, 6)
            }
            .refreshable { await projects.refresh() }
        }
        .navigationTitle("Projects")
        .navigationBarTitleDisplayMode(.large)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    showSettings = true
                } label: {
                    Image(systemName: "gearshape")
                        .foregroundStyle(ArbosTheme.textMuted)
                }
            }
        }
        .sheet(isPresented: $showSettings, onDismiss: { Task { await projects.refresh() } }) {
            SettingsView().environmentObject(settings)
        }
        .task { await projects.refresh() }
    }

    /// The open chat's project spins while any of its agents works; the
    /// others have no live link, so they rest.
    private func isWorking(_ entry: ProjectEntry) -> Bool {
        entry.target == settings.kernelTarget && chat.mode == .live && (chat.busy || chat.running > 0)
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 8) {
            if projects.loading {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small).tint(ArbosTheme.textFaint)
                    Text("Asking the hub…").foregroundStyle(ArbosTheme.textFaint)
                }
            } else {
                Text(projects.problem ?? "No projects yet.")
                    .foregroundStyle(ArbosTheme.textFaint)
            }
        }
        .font(ArbosTheme.callout)
        .padding(.top, 24)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// One project: the desktop's tab, laid flat. 30 pt glyph cell, the name
/// at body size, the machine and folder dim beneath; a dot says a kernel
/// serves it now.
struct ProjectRow: View {
    let entry: ProjectEntry
    var working = false

    var body: some View {
        HStack(spacing: 12) {
            ProjectGlyph(identity: entry.identity, size: 34, working: working)
            VStack(alignment: .leading, spacing: 2) {
                Text(entry.title)
                    .font(ArbosTheme.bodyMedium)
                    .foregroundStyle(entry.live ? ArbosTheme.text : ArbosTheme.textMuted)
                    .lineLimit(1)
                Text(subtitle)
                    .font(ArbosTheme.caption)
                    .foregroundStyle(ArbosTheme.textFaint)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            Circle()
                .fill(entry.live ? ArbosTheme.ok : ArbosTheme.textDim.opacity(0.5))
                .frame(width: 6, height: 6)
            Image(systemName: "chevron.right")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(ArbosTheme.textDim)
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 10)
        .background(
            RoundedRectangle(cornerRadius: ArbosTheme.cardRadius, style: .continuous)
                .fill(Color.clear)
        )
        .contentShape(Rectangle())
        .overlay(alignment: .bottom) {
            Rectangle().fill(ArbosTheme.border).frame(height: 0.5).padding(.leading, 56)
        }
    }

    private var subtitle: String {
        if entry.place.isEmpty { return entry.live ? entry.machine : "\(entry.machine) · no kernel running" }
        let short = entry.place.replacingOccurrences(of: "/Users/[^/]+", with: "~", options: .regularExpression)
            .replacingOccurrences(of: "/home/[^/]+", with: "~", options: .regularExpression)
        return entry.live ? "\(entry.machine) · \(short)" : "\(entry.machine) · \(short) · off"
    }
}
