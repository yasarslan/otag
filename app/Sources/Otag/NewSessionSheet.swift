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
    let provider: APIProviderInfo
    /// Start an app-owned browser in the chosen project.
    let startBrowser: (String) -> Void
    /// Called with the pane the daemon started. What the session is called
    /// comes from the daemon, which records the first prompt itself.
    let started: (PaneID) -> Void
    let teamStarted: ([PaneID]) -> Void

    @Environment(\.dismiss) private var dismiss

    /// The agent the sheet offers: whichever was used last. Someone running
    /// Codex all afternoon should not have to pick it every time. The empty
    /// string means the shell, which `AgentKind(rawValue:)` reads as "no
    /// agent" — the same thing the picker calls it.
    @AppStorage(Preference.lastAgent) private var lastAgent = AgentKind.claude.rawValue

    @State private var folder: URL?
    @State private var kind = ""
    private var agent: AgentKind? { AgentKind(rawValue: kind) }
    private var browser: Bool { kind == "browser" }
    @State private var chosen = false
    @State private var worktree = false
    @State private var model = ""
    @State private var prompt = ""
    @State private var problem: String?
    @State private var starting = false
    @State private var team = false
    @State private var createdTeam: UInt64?
    @State private var roles: [TeamRole] = []

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(Strings.t("new.title"))
                .font(.headline)
                .padding(.horizontal, 20)
                .padding(.top, 20)
                .padding(.bottom, 12)

            Form {
                Picker(Strings.t("team.mode"), selection: $team) {
                    Text(Strings.t("team.solo")).tag(false)
                    Text(Strings.t("team.title")).tag(true)
                }.pickerStyle(.segmented)
                Section {
                    LabeledContent(Strings.t("new.project")) {
                        HStack(spacing: 8) {
                            Text(folder?.path ?? Strings.t("new.project.choose"))
                                .lineLimit(1)
                                .truncationMode(.head)
                                .foregroundStyle(folder == nil ? .secondary : .primary)
                            Spacer(minLength: 0)
                            Button(Strings.t("new.project.button"), action: choose)
                        }
                    }

                    if !team {
                    Picker(Strings.t("new.runs"), selection: $kind) {
                        Text(Strings.t("new.runs.claude")).tag(AgentKind.claude.rawValue)
                        Text(Strings.t("new.runs.codex")).tag(AgentKind.codex.rawValue)
                        Text(AgentKind.deepseek.label).tag(AgentKind.deepseek.rawValue)
                            .disabled(!provider.configured)
                        Text(Strings.t("new.runs.shell")).tag("")
                        Text(Strings.t("browser.title")).tag("browser")
                    }

                    Toggle(isOn: $worktree) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(Strings.t("new.worktree"))
                            Text(Strings.t("new.worktree.detail"))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .disabled(folder == nil || browser)
                    }
                }

                if team {
                    Section(Strings.t("team.roles")) {
                        ForEach(roles.indices, id: \.self) { index in
                            VStack(alignment: .leading) {
                                if index < 3 { Text(roles[index].name) }
                                else { TextField(Strings.t("team.role.name"), text: $roles[index].name) }
                                Picker(Strings.t("new.runs"), selection: $roles[index].agent) {
                                    Text(Strings.t("new.runs.claude")).tag(AgentKind?.some(.claude))
                                    Text(Strings.t("new.runs.codex")).tag(AgentKind?.some(.codex))
                                    Text(AgentKind.deepseek.label).tag(AgentKind?.some(.deepseek))
                                        .disabled(!provider.configured)
                                    Text(Strings.t("new.runs.shell")).tag(AgentKind?.none)
                                }
                                if roles[index].agent == nil {
                                    Text(Strings.t("team.shell.note")).font(.caption).foregroundStyle(.secondary)
                                } else if index > 2 {
                                    Toggle(Strings.t("team.canEdit"), isOn: $roles[index].canEdit)
                                }
                                Toggle(Strings.t("new.worktree"), isOn: $roles[index].worktree)
                                if index > 2 {
                                    Button(Strings.t("team.remove")) { roles.remove(at: index) }
                                }
                            }
                        }
                        Button(Strings.t("team.add")) { roles.append(TeamRole(name: "")) }
                    }
                } else {
                    Section {
                        TextField(
                            Strings.t("new.model"),
                            text: $model,
                            prompt: Text(Strings.t("new.model.default"))
                        )
                        .disabled(agent == nil)
                    }
                }
                Section {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(Strings.t(team ? "team.task" : "new.prompt"))
                        TextEditor(text: $prompt)
                            .font(.body)
                            .accessibilityLabel(Strings.t(team ? "team.task" : "new.prompt"))
                            .frame(minHeight: 70)
                            .overlay {
                                RoundedRectangle(cornerRadius: 5)
                                    .strokeBorder(.separator)
                            }
                            .disabled(!team && agent == nil)
                        if team && trimmedPrompt.isEmpty {
                            Text(Strings.t("team.taskRequired"))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                if !provider.configured {
                    Text(Strings.t("api.needKey")).font(.caption).foregroundStyle(.secondary)
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
                Button(Strings.t("sheet.cancel"), role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button(Strings.t(starting ? "new.starting" : "new.start")) { start() }
                    .keyboardShortcut(.defaultAction)
                    .disabled((folder == nil && (!browser || team)) || starting
                        || (team && trimmedPrompt.isEmpty)
                        || (!team && agent == .deepseek && !provider.configured)
                        || (team && roles.contains { $0.agent == .deepseek } && !provider.configured))
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
        kind = lastAgent
        roles = [TeamRole(name: Strings.t("team.plan")),
                 TeamRole(name: Strings.t("team.coder"), canEdit: true),
                 TeamRole(name: Strings.t("team.review"))]
    }

    /// Ask for the project folder.
    private func choose() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = Strings.t("new.project.prompt")
        panel.message = Strings.t("new.project.panel")
        if panel.runModal() == .OK {
            folder = panel.url
            problem = nil
        }
    }

    private var trimmedPrompt: String { prompt.trimmingCharacters(in: .whitespacesAndNewlines) }

    /// Tell the daemon to start it, and hand the pane back.
    private func start() {
        if browser && !team {
            startBrowser(folder?.path ?? "")
            dismiss()
            return
        }
        guard let folder, !team || !trimmedPrompt.isEmpty else { return }
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
            prompt: trimmedPrompt,
            worktree: worktree
        )

        Task {
            do {
                // A connection of its own, used for this one question: the
                // store's connection is busy carrying events.
                let connection = try DaemonConnection(socket: socket)
                defer { connection.close() }
                try connection.send(team
                    ? .startTeam(cwd: folder.path, roles: roles, task: trimmedPrompt)
                    : .startSession(asked))

                for await response in connection.responses {
                    switch response {
                    case let .sessionStarted(pane, _, _):
                        if team {
                            createdTeam = pane
                            try connection.send(.listPanes)
                            continue
                        }
                        started(pane)
                        dismiss()
                        return
                    case let .panes(panes) where team:
                        if let createdTeam {
                            teamStarted(TeamMembership.members(of: createdTeam, in: panes).map(\.id))
                        }
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
