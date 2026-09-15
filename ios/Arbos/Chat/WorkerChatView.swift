import SwiftUI

/// A worker's chat, opened from its line in the project chat: the
/// desktop's agent style — the brief as the first card, then every tool
/// line and reply. Read once from the kernel's history; no composer, as
/// follow-ups belong to the root.
struct WorkerChatView: View {
    let worker: WorkerStatus
    let project: ProjectIdentity
    @EnvironmentObject private var chat: ChatStore
    @State private var items: [ChatItem] = []
    @State private var loading = true

    private var live: WorkerStatus? {
        chat.workers.first { $0.id == worker.id }
    }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: ArbosTheme.itemGap) {
                if loading {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small).tint(ArbosTheme.textFaint)
                        Text("Reading the transcript…")
                    }
                    .font(ArbosTheme.callout)
                    .foregroundStyle(ArbosTheme.textFaint)
                    .padding(.top, 12)
                } else if items.isEmpty {
                    Text("Nothing on record yet.")
                        .font(ArbosTheme.callout)
                        .foregroundStyle(ArbosTheme.textFaint)
                        .padding(.top, 12)
                }
                ForEach(items) { item in
                    ChatRow(item: item)
                }
                if let live, live.running {
                    WorkingLine(step: live.step)
                }
                Text(footer)
                    .font(ArbosTheme.caption)
                    .foregroundStyle(ArbosTheme.textDim)
                    .padding(.top, 10)
            }
            .padding(.horizontal, ArbosTheme.gutter)
            .padding(.vertical, 8)
        }
        .background(ArbosTheme.bg.ignoresSafeArea())
        .navigationTitle(worker.name)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(ArbosTheme.bg, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .task {
            items = await chat.history(agent: worker.id)
            loading = false
        }
    }

    private var footer: String {
        if let live, live.running { return "Follow-ups go to the project chat." }
        return "Done. Follow-ups aren't available for this worker."
    }
}
