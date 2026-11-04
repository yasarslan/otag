import OtagKit
import SwiftUI

/// The sessions, grouped by project.
///
/// Calm by design, like the rest of otag: a session that wants the user is
/// marked with a small symbol in the accent colour, and nothing moves,
/// blinks or fills up.
struct SidebarView: View {
    let store: SessionStore
    @Binding var selection: PaneID?
    /// Everything there is to do with one session, for the right-click menu.
    /// The same actions the session's own header offers, in the same words.
    let commands: (PaneInfo) -> SessionCommands

    var body: some View {
        List(selection: $selection) {
            // Above the sessions, because a question is the one thing in
            // this window that is actually waiting for the user.
            if !store.asks.isEmpty {
                Section("Waiting for you") {
                    InboxView(store: store) { pane in selection = pane }
                }
            }

            ForEach(store.workspaces) { workspace in
                Section {
                    ForEach(workspace.panes) { pane in
                        SessionRow(pane: pane)
                            .tag(pane.id)
                            .draggable(DraggedSession(pane: pane.id)) {
                                // What the user sees under the cursor: the
                                // session's own name, so it is obvious what
                                // is being moved.
                                SessionRow(pane: pane)
                                    .padding(6)
                                    .frame(width: 200, alignment: .leading)
                            }
                            .contextMenu {
                                SessionActionItems(commands: commands(pane))
                            }
                    }
                } header: {
                    WorkspaceHeader(workspace: workspace)
                }
            }
        }
        .listStyle(.sidebar)
        .overlay {
            if store.workspaces.isEmpty {
                EmptySidebar(state: store.state)
            }
        }
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 0) {
                if let problem = store.answerProblem {
                    AnswerProblemView(problem: problem) { store.clearAnswerProblem() }
                }
                Divider()
                UsageWidget(store: store)
                ConnectionFooter(store: store)
            }
            .background(.bar)
        }
    }
}

/// One project's name, and whether anything in it wants the user.
private struct WorkspaceHeader: View {
    let workspace: WorkspaceGroup

    var body: some View {
        HStack(spacing: 6) {
            Text(workspace.name)
            if workspace.waiting > 0 {
                Image(systemName: "questionmark.circle.fill")
                    .foregroundStyle(.tint)
                    .help("\(workspace.waiting) waiting for you")
            }
        }
    }
}

/// One session: what it is running, and what it is doing.
private struct SessionRow: View {
    let pane: PaneInfo

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: icon)
                .foregroundStyle(pane.status.wantsAttention ? AnyShapeStyle(.tint)
                    : AnyShapeStyle(.secondary))
                .frame(width: 16)

            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .lineLimit(1)
                Text(pane.status.label)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                // A context meter is only honest when otag knows how large
                // the model's context is.
                if let window = pane.contextWindow, window > 0, pane.tokens.context > 0 {
                    Meter(
                        fraction: Double(pane.tokens.context) / Double(window),
                        colour: .secondary
                    )
                    .frame(width: 90)
                }
            }
        }
        .padding(.vertical, 2)
        .help(help)
    }

    /// The agent's own symbol where there is one, and the pane's otherwise.
    private var icon: String {
        if pane.status == .error { return "exclamationmark.triangle" }
        if pane.status == .looping { return "arrow.triangle.2.circlepath" }
        if pane.status == .needsApproval { return "questionmark.circle" }
        switch pane.agent {
        case .claude, .codex: return "sparkles"
        case nil: return "terminal"
        }
    }

    private var title: String {
        if !pane.title.isEmpty { return pane.title }
        return pane.agent?.label ?? "shell"
    }

    private var help: String {
        var parts = [title, pane.status.label]
        if let agent = pane.agent { parts.insert(agent.label, at: 1) }
        return parts.joined(separator: " · ")
    }
}

/// What the sidebar says when the daemon reports no sessions at all.
private struct EmptySidebar: View {
    let state: ConnectionState

    var body: some View {
        VStack(spacing: 8) {
            if case .connecting = state {
                ProgressView().controlSize(.small)
                Text("Looking for the daemon…")
            } else {
                Text("No sessions")
                    .font(.headline)
                Text("Press ⌘N to start one, or run `otag` in a terminal.")
                    .font(.caption)
                    .multilineTextAlignment(.center)
            }
        }
        .foregroundStyle(.secondary)
        .padding(20)
    }
}

/// The bottom of the sidebar: whether the app is talking to the daemon.
///
/// The usage widget belongs here too and arrives with the rest of the usage
/// work; until then this is where the connection is reported, which is the
/// one thing a user cannot otherwise see.
///
/// The menu bar item carries the same numbers for when the window is closed.
private struct ConnectionFooter: View {
    let store: SessionStore

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(colour)
                .frame(width: 7, height: 7)
            Text(label)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .help(store.socket.path)
    }

    private var colour: Color {
        switch store.state {
        case .connected: return .green
        case .connecting: return .orange
        case .failed: return .red
        }
    }

    private var label: String {
        switch store.state {
        case .connected:
            if let version = store.daemonVersion {
                return "daemon \(version)"
            }
            return "connected"
        case .connecting:
            return "connecting…"
        case let .failed(message):
            return message
        }
    }
}
