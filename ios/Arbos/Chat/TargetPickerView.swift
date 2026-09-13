import SwiftUI

/// Which kernel to talk to: the pod, or any live project on a machine the
/// hub knows about. Tapping a row switches the main chat there.
struct TargetPickerView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var chat: ChatStore
    @Environment(\.dismiss) private var dismiss

    @State private var machines: [HubMachine] = []
    @State private var loading = false
    @State private var problem: String?

    var body: some View {
        NavigationStack {
            List {
                Section {
                    row(label: "pod", detail: "phone kernel", live: true, target: .pod)
                }
                if settings.hubConfigured {
                    Section {
                        if loading, machines.isEmpty {
                            HStack {
                                ProgressView().controlSize(.small)
                                Text("Asking the hub…").foregroundStyle(.secondary)
                            }
                        } else if let problem {
                            Text(problem).foregroundStyle(.secondary)
                        } else if machines.isEmpty {
                            Text("No machines registered.").foregroundStyle(.secondary)
                        }
                        ForEach(machines) { machine in
                            ForEach(machine.projects) { project in
                                row(
                                    label: "\(machine.name)/\(project.name)",
                                    detail: project.live ? (project.place.isEmpty ? machine.host : project.place) : "no kernel running",
                                    live: project.live,
                                    target: .hub(machine: machine.name, project: project.name)
                                )
                            }
                            if machine.projects.isEmpty {
                                row(
                                    label: machine.name,
                                    detail: machine.worker ? "worker only" : "nothing served",
                                    live: false,
                                    target: .hub(machine: machine.name, project: "")
                                )
                            }
                        }
                    } header: {
                        Text("Hub")
                    } footer: {
                        Text(settings.hubURL)
                    }
                } else {
                    Section {
                        Text("Set the hub URL and token in Settings to see other machines.")
                            .foregroundStyle(.secondary)
                    } header: {
                        Text("Hub")
                    }
                }
            }
            .navigationTitle("Kernel")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { dismiss() }
                }
                ToolbarItem(placement: .primaryAction) {
                    Button {
                        Task { await load() }
                    } label: {
                        Image(systemName: "arrow.clockwise")
                    }
                    .disabled(loading || !settings.hubConfigured)
                }
            }
            .task { await load() }
        }
        .preferredColorScheme(.dark)
    }

    private func row(label: String, detail: String, live: Bool, target: KernelTarget) -> some View {
        Button {
            Task {
                await chat.switchTarget(target)
                dismiss()
            }
        } label: {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(label)
                        .font(.body.monospaced())
                        .foregroundStyle(live ? .primary : .secondary)
                    if !detail.isEmpty {
                        Text(detail)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                Spacer()
                if settings.kernelTarget == target {
                    Image(systemName: "checkmark").foregroundStyle(.tint)
                }
            }
        }
        .disabled(!live)
    }

    private func load() async {
        guard settings.hubConfigured else { return }
        loading = true
        problem = nil
        do {
            machines = try await HubClient.list(hubURL: settings.hubURL, token: settings.hubToken)
        } catch {
            problem = error.localizedDescription
        }
        loading = false
    }
}
