import SwiftUI

/// A worker's chat, opened from its line or the Agents pill: the
/// desktop's agent style — the brief as the first card, then every tool
/// line and reply. Read once from the kernel's history; no composer, as
/// follow-ups belong to the project chat.
struct WorkerChatView: View {
    let worker: WorkerStatus
    let project: ProjectIdentity
    @EnvironmentObject private var chat: ChatStore
    @Environment(\.dismiss) private var dismiss
    @State private var items: [ChatItem] = []
    @State private var loading = true

    private var live: WorkerStatus? {
        chat.workers.first { $0.id == worker.id }
    }

    var body: some View {
        VStack(spacing: 0) {
            topBar
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
                    Color.clear.frame(height: 24)
                }
                .padding(.horizontal, ArbosTheme.gutter + 6)
                .padding(.top, 4)
            }
        }
        .background(ArbosTheme.bg.ignoresSafeArea())
        .toolbar(.hidden, for: .navigationBar)
        .task {
            items = await chat.history(agent: worker.id)
            loading = false
        }
    }

    private var topBar: some View {
        ZStack {
            HStack(spacing: 8) {
                if let live, live.running {
                    BrailleSpinner(tint: project.tint)
                        .font(.system(size: 12, design: .monospaced))
                } else {
                    Image(systemName: "checkmark")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(ArbosTheme.textFaint)
                }
                Text(worker.name)
                    .font(ArbosTheme.bodyMedium)
                    .foregroundStyle(ArbosTheme.text)
                    .lineLimit(1)
            }
            .frame(maxWidth: 220)
            HStack {
                RoundButton(symbol: "chevron.left") { dismiss() }
                Spacer()
            }
        }
        .padding(.horizontal, ArbosTheme.gutter + 6)
        .padding(.top, 6)
        .padding(.bottom, 10)
    }

    private var footer: String {
        if let live, live.running { return "Follow-ups go to the project chat." }
        return "Done. Follow-ups aren't available for this worker."
    }
}
