import AppKit
import OtagKit
import SwiftUI

/// Starting a session: where it works, what runs in it, and what to ask it
/// first.
///
/// Only the folder is required. Everything else has a sensible default, so
/// the sheet is one choice and a return key away from a running agent.
struct NewSessionSheet: View {
    /// Where the daemon is, so the sheet can ask it to start the session.
    let socket: URL
    /// The project to offer: the one the user is looking at, or the newest
    /// session's. Nothing when otag is running nothing at all.
    let suggestedProject: String?
    /// Called with the pane the daemon started and the first prompt it was
    /// given, which is what otag names the session after.
    let started: (PaneID, String) -> Void

    @Environment(\.dismiss) private var dismiss

    /// The agent the sheet offers: whichever was used last. Someone running
    /// Codex all afternoon should not have to pick it every time. The empty
    /// string means the shell, which `AgentKind(rawValue:)` reads as "no
    /// agent" — the same thing the picker calls it.
    @AppStorage(Preference.lastAgent) private var lastAgent = AgentKind.claude.rawValue

    @State private var folder: URL?
    @State private var agent: AgentKind?
    @State private var chosen = false
    @State private var worktree = false
    @State private var model = ""
    @State private var prompt = ""
    @State private var problem: String?
    @State private var starting = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("New session")
                .font(.headline)
                .padding(.horizontal, 20)
                .padding(.top, 20)
                .padding(.bottom, 12)

            Form {
                Section {
                    LabeledContent("Project") {
                        HStack(spacing: 8) {
                            Text(folder?.path ?? "Choose a folder…")
                                .lineLimit(1)
                                .truncationMode(.head)
                                .foregroundStyle(folder == nil ? .secondary : .primary)
                            Spacer(minLength: 0)
                            Button("Choose…", action: choose)
                        }
                    }

                    Picker("Runs", selection: $agent) {
                        Text("Claude Code").tag(AgentKind?.some(.claude))
                        Text("Codex").tag(AgentKind?.some(.codex))
                        Text("Shell").tag(AgentKind?.none)
                    }

                    Toggle(isOn: $worktree) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Give it a git worktree of its own")
                            Text("A branch and a directory to itself, so it cannot "
                                + "collide with the other sessions in this checkout.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .disabled(folder == nil)
                }

                Section {
                    TextField("Model", text: $model, prompt: Text("the agent's default"))
                        .disabled(agent == nil)
                    VStack(alignment: .leading, spacing: 4) {
                        Text("First prompt")
                        TextEditor(text: $prompt)
                            .font(.body)
                            .frame(minHeight: 70)
                            .overlay {
                                RoundedRectangle(cornerRadius: 5)
                                    .strokeBorder(.separator)
                            }
                            .disabled(agent == nil)
                    }
                }
            }
            .formStyle(.grouped)

            if let problem {
                Label(problem, systemImage: "exclamationmark.triangle")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 20)
                    .padding(.bottom, 8)
            }

            Divider()
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button(starting ? "Starting…" : "Start") { start() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(folder == nil || starting)
            }
            .padding(16)
        }
        .frame(width: 520)
        .onAppear(perform: prefill)
    }

    /// Fill the sheet in with what the user almost certainly means.
    ///
    /// Only once: re-running this would undo a folder the user has just
    /// chosen if the view were rebuilt.
    private func prefill() {
        guard !chosen else { return }
        chosen = true
        if let suggestedProject, !suggestedProject.isEmpty {
            folder = URL(fileURLWithPath: suggestedProject, isDirectory: true)
        }
        agent = AgentKind(rawValue: lastAgent)
    }

    /// Ask for the project folder.
    private func choose() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Choose"
        panel.message = "Choose the project this session works in"
        if panel.runModal() == .OK {
            folder = panel.url
            problem = nil
        }
    }

    /// Tell the daemon to start it, and hand the pane back.
    private func start() {
        guard let folder else { return }
        starting = true
        problem = nil
        // Remembered on starting rather than on choosing, so a sheet that
        // was cancelled changes nothing. The empty string is the shell,
        // which is as much a choice as either agent.
        lastAgent = agent?.rawValue ?? ""

        let asked = StartSession(
            cwd: folder.path,
            agent: agent,
            model: model.trimmingCharacters(in: .whitespacesAndNewlines),
            prompt: prompt.trimmingCharacters(in: .whitespacesAndNewlines),
            worktree: worktree
        )
        let firstPrompt = asked.prompt ?? ""

        Task {
            do {
                // A connection of its own, used for this one question: the
                // store's connection is busy carrying events.
                let connection = try DaemonConnection(socket: socket)
                defer { connection.close() }
                try connection.send(.startSession(asked))

                for await response in connection.responses {
                    switch response {
                    case let .sessionStarted(pane, _, _):
                        started(pane, firstPrompt)
                        dismiss()
                        return
                    case let .error(message):
                        problem = message
                        starting = false
                        return
                    default:
                        continue
                    }
                }
                problem = DaemonError.disconnected.localizedDescription
                starting = false
            } catch {
                problem = error.localizedDescription
                starting = false
            }
        }
    }
}
