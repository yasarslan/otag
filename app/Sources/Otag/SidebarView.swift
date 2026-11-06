import OtagKit
import SwiftUI

/// The sessions, grouped by project. The only navigator in the window.
///
/// There is no tab bar and no quick switcher: a session is here, you click
/// it, and the main area shows it. A row that is waiting for the user opens
/// up under its own name with the question and two buttons, because a
/// question belongs to the session that asked it rather than to an inbox
/// somewhere else.
struct SidebarView: View {
    let store: SessionStore
    let names: SessionNames
    let workbench: Workbench
    let browsers: BrowserSessions
    @Binding var selection: PaneID?
    /// Ask the window something about one session.
    let ask: (PaneID, SessionPrompt.Kind) -> Void

    /// The session whose nickname is being typed, if any.
    ///
    /// Edited in the row rather than in a sheet: a nickname is one word, and
    /// a dialog for one word is a dialog too many.
    @State private var naming: PaneID?

    var body: some View {
        List(selection: $selection) {
            ForEach(store.workspaces) { workspace in
                Section {
                    ForEach(workspace.panes.filter { $0.team == nil }) { pane in
                        row(for: pane)
                    }
                    ForEach(Array(Set(workspace.panes.compactMap { $0.team?.id })).sorted(), id: \.self) { team in
                        DisclosureGroup(Strings.t("team.title") + " \(team)") {
                            ForEach(workspace.panes.filter { $0.team?.id == team }.sorted { ($0.team?.index ?? 0) < ($1.team?.index ?? 0) }) { pane in
                                row(for: pane)
                            }
                        }
                    }
                } header: {
                    ProjectHeader(name: workspace.name)
                }
            }
            if !browsers.sessions.isEmpty {
                Section(Strings.t("browser.title")) {
                    ForEach(browsers.sessions) { browser in
                        Label(browser.url.absoluteString, systemImage: "globe")
                            .lineLimit(1)
                            .tag(browser.id)
                            .draggable(DraggedSession(pane: browser.id))
                            .contextMenu {
                                Button(Strings.t("menu.closeSession")) { browsers.close(browser.id) }
                            }
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
        .chromeBackground()
        .overlay {
            if store.workspaces.isEmpty && browsers.sessions.isEmpty {
                EmptySidebar(state: store.state)
            }
        }
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 0) {
                if let problem = store.answerProblem {
                    AnswerProblemView(problem: problem) { store.clearAnswerProblem() }
                }
                LimitsCard(store: store, focused: workbench.focus.flatMap(store.pane))
            }
        }
    }

    /// One row, with everything hung off it: the drag that puts a session
    /// side by side, and the right click that does everything else.
    private func row(for pane: PaneInfo) -> some View {
        SessionRow(
            pane: pane,
            name: names.name(of: pane),
            number: number(of: pane.id),
            ask: store.asks.first { $0.pane == pane.id },
            naming: naming == pane.id,
            answer: { chosen in deliver(chosen, for: pane.id) },
            rename: { nickname in
                naming = nil
                actions(for: pane).setNickname(nickname)
            },
            cancel: { naming = nil }
        )
        .tag(pane.id)
        .draggable(DraggedSession(pane: pane.id)) { dragPreview(of: pane) }
        .contextMenu {
            SessionContextMenu(
                actions: actions(for: pane),
                everything: store.panes,
                ask: { kind in ask(pane.id, kind) },
                setNickname: { naming = pane.id }
            )
        }
    }

    /// What the user sees under the cursor: the session's own name, so it is
    /// obvious what is being moved.
    private func dragPreview(of pane: PaneInfo) -> some View {
        HStack(spacing: Chrome.gap) {
            AgentMark(agent: pane.agent)
            Text(names.name(of: pane)).lineLimit(1)
        }
        .padding(8)
        .frame(width: 200, alignment: .leading)
    }

    private func deliver(_ answer: Answer, for pane: PaneID) {
        guard let waiting = store.asks.first(where: { $0.pane == pane }) else { return }
        store.answer(waiting, with: answer)
    }

    /// Everything there is to do with one session.
    private func actions(for pane: PaneInfo) -> SessionActions {
        SessionActions(store: store, workbench: workbench, names: names, pane: pane)
    }

    /// Which `⌘` number selects this session, when it is one of the first
    /// nine. Shown in the row so the shortcut teaches itself.
    private func number(of pane: PaneID) -> Int? {
        guard let index = store.ordered.firstIndex(of: pane), index < 9 else { return nil }
        return index + 1
    }
}

/// One project's name: a label, not a control.
///
/// No disclosure triangle, no count, no icon. Projects are how the list is
/// grouped; they are not things you do anything to.
private struct ProjectHeader: View {
    let name: String

    var body: some View {
        Text(name)
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(.secondary)
            .lineLimit(1)
            .truncationMode(.middle)
    }
}

/// One session: a colour for the agent, a name, and what it is doing.
///
/// Three things, and a fourth only when the session is waiting: the question,
/// with Allow and Deny under it.
private struct SessionRow: View {
    let pane: PaneInfo
    let name: String
    /// Its `⌘` number, when it has one.
    let number: Int?
    /// What it is waiting for, when it is waiting for anything.
    let ask: AskView?
    /// Whether the nickname is being typed right now.
    let naming: Bool
    let answer: (Answer) -> Void
    let rename: (String) -> Void
    let cancel: () -> Void

    @State private var typed = ""
    @FocusState private var typing: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: Chrome.gap) {
                AgentMark(agent: pane.agent)

                Text(name)
                    .lineLimit(1)
                    .truncationMode(.middle)

                Spacer(minLength: 4)

                if guarded {
                    Image(systemName: "shield.fill")
                        .font(.system(size: 9))
                        .foregroundStyle(held ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
                        .help(guardHelp)
                }

                StatusPill(status: pane.status)
            }

            // The nickname: what job this session is doing, under what it
            // was asked. Only a line when there is one, so a sidebar of
            // sessions nobody has nicknamed looks exactly as it did.
            if naming {
                TextField(Strings.t("sidebar.nickname"), text: $typed)
                    .textFieldStyle(.roundedBorder)
                    .controlSize(.small)
                    .font(.caption)
                    .focused($typing)
                    .onSubmit { rename(typed) }
                    .onExitCommand(perform: cancel)
                    .padding(.leading, 16)
                    .onAppear {
                        typed = pane.nickname
                        typing = true
                    }
            } else if !pane.nickname.isEmpty {
                Text(pane.nickname)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .padding(.leading, 16)
            }

            // The one thing in this window that is actually waiting for the
            // user, under the name of whoever is asking.
            if let ask {
                VStack(alignment: .leading, spacing: 6) {
                    Text(ask.summary)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(3)
                        .fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 6) {
                        Button(Strings.t("ask.allow")) { answer(.allow) }
                            .controlSize(.small)
                            .keyboardShortcut(.defaultAction)
                        Button(Strings.t("ask.deny")) { answer(.deny) }
                            .controlSize(.small)
                        Spacer(minLength: 0)
                    }
                }
                .padding(.leading, 16)
                .transition(.opacity)
            }
        }
        .padding(.vertical, 3)
        .animation(.easeOut(duration: 0.15), value: ask?.id)
        .animation(.easeOut(duration: 0.15), value: naming)
        .help(help)
    }

    /// Whether the limit guard is watching this session.
    private var guarded: Bool {
        pane.guardPlan != nil
    }

    /// Whether the guard has actually stopped it, rather than only watching.
    private var held: Bool {
        guard let plan = pane.guardPlan else { return false }
        return plan.paused || plan.handedOff
    }

    private var guardHelp: String {
        guard let plan = pane.guardPlan else { return "" }
        if plan.handedOff {
            return Strings.t("guard.help.handed")
        }
        if plan.paused {
            guard let resets = Readable.resetClock(plan.resetsAt) else {
                return Strings.t("guard.help.stopped")
            }
            return Strings.t("guard.help.stopped.until", resets)
        }
        return Strings.t("guard.help.watching", Int(plan.threshold))
    }

    private var help: String {
        var parts = [name]
        if !pane.nickname.isEmpty { parts.append(pane.nickname) }
        parts.append(pane.agent.name)
        parts.append(pane.status.localizedPrecise)
        if let number { parts.append("⌘\(number)") }
        return parts.joined(separator: " · ")
    }
}

/// What the sidebar says when the daemon reports no sessions at all.
private struct EmptySidebar: View {
    let state: ConnectionState

    var body: some View {
        VStack(spacing: Chrome.gap) {
            if case .connecting = state {
                ProgressView().controlSize(.small)
                Text(Strings.t("sidebar.looking"))
            } else {
                Text(Strings.t("sidebar.empty.title"))
                    .font(.headline)
                Text(Strings.t("sidebar.empty.detail"))
                    .font(.caption)
                    .multilineTextAlignment(.center)
            }
        }
        .foregroundStyle(.secondary)
        .padding(20)
    }
}
