import OtagKit
import SwiftUI

/// A session the user has asked for but the daemon has not started yet.
///
/// It carries where the new session should go: on its own, or beside the one
/// that is showing.
struct PendingSession: Identifiable {
    let id = UUID()
    /// How to place the new session, or `nil` for on its own.
    var axis: SplitAxis?
}

/// Otag: a native window onto the otag daemon.
///
/// The daemon owns the panes, the agents and everything known about them;
/// this app connects to it, shows what it reports, and sends what the user
/// does. Quitting the app leaves every agent running.
///
/// One mental model: **sessions**. The sidebar lists them, the main area
/// shows the selected one or up to three side by side, and there is nothing
/// else to learn.
@main
struct OtagApp: App {
    @State private var store = SessionStore()
    @State private var workbench = Workbench()
    @State private var names = SessionNames()
    @State private var pending: PendingSession?
    @State private var attention = Attention()
    @State private var palette = false
    @AppStorage("hasOnboarded") private var onboarded = false

    var body: some Scene {
        WindowGroup {
            MainView(
                store: store,
                workbench: workbench,
                names: names,
                pending: $pending,
                palette: $palette,
                onboarded: $onboarded
            )
            .task {
                attention.prepare()
                store.start()
            }
            // The Dock badge and the notifications follow the inbox, which
            // the daemon keeps in step with the panes themselves.
            .onChange(of: store.asks) { _, asks in
                attention.update(asks: asks, waiting: store.waiting, store: store)
            }
            .onChange(of: store.waiting) { _, waiting in
                attention.update(asks: store.asks, waiting: waiting, store: store)
            }
            .onChange(of: attention.pending?.ask) { _, _ in
                guard let answered = attention.pending else { return }
                attention.pending = nil
                guard let ask = store.asks.first(where: { $0.id == answered.ask })
                else { return }
                store.answer(ask, with: answered.answer)
            }
        }
        .defaultSize(width: 1_100, height: 700)
        .windowToolbarStyle(.unified)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("New Session…") { pending = PendingSession(axis: nil) }
                    .keyboardShortcut("n", modifiers: .command)
            }
            // Cmd-K is for whoever already knows it is there. It is not on
            // the toolbar and not in the welcome: the menus and the sidebar
            // are the app, and a palette is a faster way to what they
            // already have.
            CommandGroup(after: .newItem) {
                Button("Command Palette…") { palette = true }
                    .keyboardShortcut("k", modifiers: .command)
            }
            CommandGroup(replacing: .help) {
                Button("Show the Welcome Screen") { onboarded = false }
            }
            CommandMenu("Session") {
                Button("Allow What Is Waiting") { answerFirst(.allow) }
                    .keyboardShortcut("y", modifiers: [.command, .shift])
                    .disabled(store.asks.isEmpty)
                Button("Deny What Is Waiting") { answerFirst(.deny) }
                    .keyboardShortcut("n", modifiers: [.command, .shift])
                    .disabled(store.asks.isEmpty)
                Divider()
                // Splitting means another session beside this one, and a
                // pane is the daemon's to create: the only honest way to get
                // one is to say what should run in it.
                Button("Split Right…") { pending = PendingSession(axis: .horizontal) }
                    .keyboardShortcut("d", modifiers: .command)
                Button("Split Down…") { pending = PendingSession(axis: .vertical) }
                    .keyboardShortcut("d", modifiers: [.command, .shift])
                Divider()
                Button("Close Session") { closeFocused() }
                    .keyboardShortcut("w", modifiers: .command)
                    .disabled(workbench.focus == nil)
                Button("Focus Next Session") { workbench.stepFocus(by: 1) }
                    .keyboardShortcut("]", modifiers: .command)
                    .disabled(workbench.panes.count < 2)
                Button("Focus Previous Session") { workbench.stepFocus(by: -1) }
                    .keyboardShortcut("[", modifiers: .command)
                    .disabled(workbench.panes.count < 2)
                Divider()
                Button(handOffTitle) { handOff() }
                    .disabled(focusedPane?.agent == nil)
                Button("Morning Report…") { store.askForMorningReport() }
                    .keyboardShortcut("r", modifiers: [.command, .shift])
            }
            // ⌘1…⌘9 go to the first nine sessions in the sidebar's order.
            // The sidebar shows each row's number, so the shortcut teaches
            // itself rather than needing a list to memorise.
            CommandMenu("Go") {
                ForEach(1 ... 9, id: \.self) { number in
                    Button(sessionName(number)) { go(to: number) }
                        .keyboardShortcut(KeyEquivalent(Character("\(number)")), modifiers: .command)
                        .disabled(store.ordered.count < number)
                }
            }
        }

        // Visible with the window closed, which is how an agent that needs
        // permission is found without going looking for the app.
        MenuBarExtra {
            MenuBarView(store: store)
        } label: {
            MenuBarLabel(store: store)
        }

        Settings {
            SettingsView()
        }
    }

    /// The session the window is showing.
    private var focusedPane: PaneInfo? {
        workbench.focus.flatMap(store.pane)
    }

    private var handOffTitle: String {
        switch focusedPane?.agent {
        case .claude: return "Switch to Codex"
        case .codex: return "Switch to Claude"
        case nil: return "Switch Agent"
        }
    }

    /// Hand the showing session over to the other agent.
    private func handOff() {
        guard let pane = focusedPane, pane.agent != nil else { return }
        store.handOff(pane: pane.id, handoff: .agent(to: nil))
    }

    /// Answer the oldest question, which is the one the sidebar shows first.
    private func answerFirst(_ answer: Answer) {
        guard let ask = store.asks.first else { return }
        store.answer(ask, with: answer)
    }

    private func closeFocused() {
        guard let pane = workbench.focus else { return }
        store.send(.detachPane(pane: pane))
        workbench.close(pane: pane)
    }

    /// Show the nth session in the sidebar's order.
    private func go(to number: Int) {
        let ordered = store.ordered
        guard ordered.indices.contains(number - 1) else { return }
        workbench.select(pane: ordered[number - 1])
    }

    /// What `⌘n` would go to, so the menu reads as sessions rather than
    /// numbers.
    private func sessionName(_ number: Int) -> String {
        let ordered = store.ordered
        guard ordered.indices.contains(number - 1),
              let pane = store.pane(ordered[number - 1])
        else { return "Session \(number)" }
        return names.name(of: pane)
    }
}

/// The window: the sessions on the left, the selected one on the right.
struct MainView: View {
    let store: SessionStore
    let workbench: Workbench
    let names: SessionNames
    @Binding var pending: PendingSession?
    @Binding var palette: Bool
    @Binding var onboarded: Bool

    /// What the window is asking about one session, when it is asking.
    @State private var prompt: SessionPrompt?

    var body: some View {
        NavigationSplitView {
            SidebarView(
                store: store,
                names: names,
                workbench: workbench,
                selection: selection,
                ask: { pane, kind in prompt = SessionPrompt(pane: pane, kind: kind) }
            )
            .navigationSplitViewColumnWidth(min: 220, ideal: 260, max: 380)
        } detail: {
            if workbench.root == nil {
                EmptyStateView(state: store.state) {
                    pending = PendingSession(axis: nil)
                }
                // With nothing showing, a session dragged anywhere here
                // opens it: there are no edges to aim at yet.
                .dropDestination(for: DraggedSession.self) { items, _ in
                    guard let dropped = items.first,
                          store.pane(dropped.pane) != nil else { return false }
                    workbench.select(pane: dropped.pane)
                    return true
                }
            } else {
                PanesView(
                    store: store,
                    names: names,
                    workbench: workbench,
                    ask: { pane, kind in prompt = SessionPrompt(pane: pane, kind: kind) }
                )
            }
        }
        .toolbar {
            // The only thing on the toolbar. Everything else about a session
            // is above the session.
            ToolbarItem(placement: .primaryAction) {
                Button {
                    pending = PendingSession(axis: nil)
                } label: {
                    Label("New session", systemImage: "plus")
                }
                .help("Start a session (⌘N)")
            }
        }
        .sheet(item: $pending) { request in
            NewSessionSheet(
                socket: store.socket,
                suggestedProject: store.suggestedProject(near: workbench.focus)
            ) { pane, prompt in
                if !prompt.isEmpty {
                    names.remember(firstPrompt: prompt, for: pane)
                }
                place(pane: pane, axis: request.axis)
            }
        }
        .sheet(item: $prompt) { asking in
            if let pane = store.pane(asking.pane) {
                SessionSheet(
                    actions: SessionActions(
                        store: store,
                        workbench: workbench,
                        names: names,
                        pane: pane
                    ),
                    kind: asking.kind,
                    everything: store.panes
                )
            }
        }
        .sheet(isPresented: $palette) {
            PaletteView(items: items()) { palette = false }
        }
        .sheet(isPresented: onboarding) {
            WelcomeView(
                socket: store.socket,
                start: {
                    onboarded = true
                    pending = PendingSession(axis: nil)
                },
                dismiss: { onboarded = true }
            )
            .interactiveDismissDisabled()
        }
        .sheet(isPresented: reportShown) {
            MorningReportView(sessions: store.morningReport ?? []) {
                store.clearMorningReport()
            }
        }
        .safeAreaInset(edge: .bottom) {
            if let notice = store.notice {
                NoticeBar(notice: notice) { store.clearNotice() }
            } else if let refusal = workbench.refusal {
                NoticeBar(notice: refusal) { workbench.clearRefusal() }
            }
        }
        .onChange(of: store.panes) { _, panes in
            // The first word from the daemon is the first chance to put back
            // the arrangement the window was left in: until the sessions are
            // known, a saved layout cannot be checked against them.
            workbench.restore(alive: panes.map(\.id))

            // A pane the daemon has let go of cannot stay on screen.
            let alive = Set(panes.map(\.id))
            for pane in workbench.panes where !alive.contains(pane) {
                workbench.close(pane: pane)
            }
            names.prune(to: panes.map(\.id))
            workbench.save()
        }
        .onChange(of: workbench.root) { _, _ in workbench.save() }
        .onChange(of: workbench.focus) { _, _ in workbench.save() }
    }

    /// The welcome screen is shown until it has been gone through once.
    private var onboarding: Binding<Bool> {
        Binding(
            get: { !onboarded },
            set: { shown in if !shown { onboarded = true } }
        )
    }

    /// Everything the palette offers, in the order it offers it.
    ///
    /// Sessions first, because jumping to one is what it is used for; the
    /// handful of actions after. Nothing is only here — every one of these
    /// is also a menu item or a button, which is what keeps the palette a
    /// shortcut rather than a place to learn.
    private func items() -> [PaletteItem] {
        var items: [PaletteItem] = store.panes.map { pane in
            PaletteItem(
                title: names.name(of: pane),
                detail: "\(pane.workspace) · \(pane.status.precise)",
                symbol: pane.agent == nil ? "terminal" : "sparkles"
            ) {
                workbench.select(pane: pane.id)
            }
        }

        items.append(PaletteItem(
            title: "New session",
            detail: "a project, an agent and a first prompt",
            shortcut: "⌘N",
            symbol: "plus"
        ) { pending = PendingSession(axis: nil) })

        if let pane = focused {
            let actions = SessionActions(
                store: store,
                workbench: workbench,
                names: names,
                pane: pane
            )
            if actions.canHandOff {
                items.append(PaletteItem(
                    title: actions.handOffTitle,
                    detail: actions.name,
                    symbol: "arrow.left.arrow.right"
                ) { actions.handOff() })
            }
            if pane.agent != nil {
                items.append(PaletteItem(
                    title: "Lighter model…",
                    detail: actions.name,
                    symbol: "arrow.down.right.circle"
                ) { prompt = SessionPrompt(pane: pane.id, kind: .model) })
                items.append(PaletteItem(
                    title: "Night shift…",
                    detail: actions.name,
                    symbol: "moon.stars"
                ) { prompt = SessionPrompt(pane: pane.id, kind: .nightShift) })
            }
            items.append(PaletteItem(
                title: "Rename…",
                detail: actions.name,
                symbol: "pencil"
            ) { prompt = SessionPrompt(pane: pane.id, kind: .rename) })
            items.append(PaletteItem(
                title: "Close",
                detail: actions.name,
                shortcut: "⌘W",
                symbol: "xmark"
            ) { actions.close() })
        }

        items.append(PaletteItem(
            title: "Morning report…",
            detail: "what otag did overnight",
            shortcut: "⇧⌘R",
            symbol: "sunrise"
        ) { store.askForMorningReport() })

        for ask in store.asks {
            items.append(PaletteItem(
                title: "Allow: \(ask.summary)",
                detail: ask.agent?.label ?? "",
                symbol: "checkmark.circle"
            ) { store.answer(ask, with: .allow) })
            items.append(PaletteItem(
                title: "Deny: \(ask.summary)",
                detail: ask.agent?.label ?? "",
                symbol: "xmark.circle"
            ) { store.answer(ask, with: .deny) })
        }

        return items
    }

    private var focused: PaneInfo? {
        workbench.focus.flatMap(store.pane)
    }

    /// The report is shown exactly while there is one.
    private var reportShown: Binding<Bool> {
        Binding(
            get: { store.morningReport != nil },
            set: { shown in if !shown { store.clearMorningReport() } }
        )
    }

    /// The sidebar's selection is the session with the keyboard, and
    /// choosing one there shows it.
    private var selection: Binding<PaneID?> {
        Binding(
            get: { workbench.focus },
            set: { pane in
                guard let pane else { return }
                workbench.select(pane: pane)
            }
        )
    }

    /// Put a freshly started session where the user asked for it.
    private func place(pane: PaneID, axis: SplitAxis?) {
        if let axis, workbench.root != nil {
            workbench.split(with: pane, axis: axis)
        } else {
            workbench.select(pane: pane)
        }
    }
}
