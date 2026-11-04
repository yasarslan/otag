import OtagKit
import SwiftUI

/// A session the user has asked for but the daemon has not started yet.
///
/// It carries where the new pane should go: a tab of its own, or beside the
/// pane that is showing.
struct PendingSession: Identifiable {
    let id = UUID()
    /// How to place the new pane, or `nil` for a tab of its own.
    var axis: SplitAxis?
}

/// Otag: a native window onto the otag daemon.
///
/// The daemon owns the panes, the agents and everything known about them;
/// this app connects to it, shows what it reports, and sends what the user
/// does. Quitting the app leaves every agent running.
@main
struct OtagApp: App {
    @State private var store = SessionStore()
    @State private var workbench = Workbench()
    @State private var pending: PendingSession?
    @State private var attention = Attention()
    @State private var palette: PaletteKind?
    /// The session whose model the user is choosing, from the menu bar.
    @State private var askingForModel: ChoosingModel?
    @AppStorage("hasOnboarded") private var onboarded = false

    var body: some Scene {
        WindowGroup {
            MainView(
                store: store,
                workbench: workbench,
                pending: $pending,
                palette: $palette,
                askingForModel: $askingForModel,
                onboarded: $onboarded
            )
                .task {
                    attention.prepare()
                    store.start()
                }
                // The Dock badge and the notifications follow the inbox,
                // which the daemon keeps in step with the panes themselves.
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
            CommandGroup(after: .newItem) {
                Button("Command Palette…") { palette = .commands }
                    .keyboardShortcut("k", modifiers: .command)
                Button("Go to Session…") { palette = .sessions }
                    .keyboardShortcut("p", modifiers: .command)
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
                Button("Close Tab") { closeTab() }
                    .keyboardShortcut("w", modifiers: .command)
                Button("Next Tab") { workbench.step(by: 1) }
                    .keyboardShortcut("]", modifiers: [.command, .shift])
                Button("Previous Tab") { workbench.step(by: -1) }
                    .keyboardShortcut("[", modifiers: [.command, .shift])
                Divider()
                Button("Hand Over to the Other Agent") { handOver() }
                    .disabled(focusedPane?.agent == nil)
                Button("Continue on a Lighter Model…") { continueOnAnotherModel() }
                    .disabled(focusedPane?.agent == nil)
                Button("Morning Report…") { store.askForMorningReport() }
                    .keyboardShortcut("r", modifiers: [.command, .shift])
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

    /// Hand the showing session over to the other agent.
    private func handOver() {
        guard let pane = focusedPane, pane.agent != nil else { return }
        store.handOver(pane: pane.id, handoff: .agent(to: nil))
    }

    /// Continue the showing session on another model.
    private func continueOnAnotherModel() {
        guard let pane = focusedPane, pane.agent != nil else { return }
        askingForModel = ChoosingModel(id: pane.id)
    }

    /// Answer the oldest question, which is the one the sidebar shows first.
    private func answerFirst(_ answer: Answer) {
        guard let ask = store.asks.first else { return }
        store.answer(ask, with: answer)
    }

    private func closeTab() {
        guard let id = workbench.selected else { return }
        for pane in workbench.closeTab(id) {
            store.send(.detachPane(pane: pane))
        }
    }
}

/// The window: sessions on the left, the open tabs on the right.
struct MainView: View {
    let store: SessionStore
    let workbench: Workbench
    @Binding var pending: PendingSession?
    @Binding var palette: PaletteKind?
    /// The session whose model the user is choosing, if any. Reached from a
    /// session's header, from a right click in the sidebar, from the menu
    /// bar and from the palette, so it belongs to the window rather than to
    /// any one of them.
    @Binding var askingForModel: ChoosingModel?
    @Binding var onboarded: Bool
    @State private var inspector = false

    var body: some View {
        NavigationSplitView {
            SidebarView(
                store: store,
                selection: selection,
                commands: commands(for:)
            )
                .navigationSplitViewColumnWidth(min: 220, ideal: 260, max: 380)
        } detail: {
            if workbench.tabs.isEmpty {
                EmptyDetailView(state: store.state) {
                    pending = PendingSession(axis: nil)
                }
            } else {
                TabsView(
                    store: store,
                    workbench: workbench,
                    pending: $pending,
                    askingForModel: $askingForModel
                )
            }
        }
        .inspector(isPresented: $inspector) {
            UsageInspector(store: store, pane: workbench.focus.flatMap(store.pane))
                .inspectorColumnWidth(min: 260, ideal: 300, max: 420)
        }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    pending = PendingSession(axis: nil)
                } label: {
                    Label("New Session", systemImage: "plus")
                }
                .help("Start a session (⌘N)")
            }
            ToolbarItem(placement: .primaryAction) {
                Button {
                    inspector.toggle()
                } label: {
                    Label("Usage", systemImage: "chart.line.uptrend.xyaxis")
                }
                .help("Usage and forecast (⌘⌥I)")
                .keyboardShortcut("i", modifiers: [.command, .option])
            }
        }
        .sheet(item: $pending) { request in
            NewSessionSheet(
                socket: store.socket,
                suggestedProject: store.suggestedProject(near: workbench.focus)
            ) { pane, title in
                place(pane: pane, title: title, axis: request.axis)
            }
        }
        .sheet(item: $askingForModel) { choosing in
            LighterModelSheet(store: store, pane: choosing.id)
        }
        .sheet(item: $palette) { kind in
            PaletteView(title: kind.title, items: items(for: kind)) { palette = nil }
        }
        .sheet(isPresented: onboarding) {
            OnboardingView(socket: store.socket) { onboarded = true }
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
            }
        }
        .onChange(of: store.panes) { _, panes in
            // A pane the daemon has let go of cannot stay on screen.
            let alive = Set(panes.map(\.id))
            for tab in workbench.tabs {
                for pane in tab.panes where !alive.contains(pane) {
                    workbench.close(pane: pane)
                }
            }
            retitleTabs(panes)
        }
    }

    /// The welcome screen is shown until it has been gone through once.
    private var onboarding: Binding<Bool> {
        Binding(
            get: { !onboarded },
            set: { shown in if !shown { onboarded = true } }
        )
    }

    /// Everything there is to do with one session, for whichever surface is
    /// offering it: a header, a right click, or the palette.
    private func commands(for pane: PaneInfo) -> SessionCommands {
        SessionCommands(
            store: store,
            workbench: workbench,
            pane: pane,
            split: { axis in
                // The split is from this session, so it has to be the one
                // with the keyboard before the new pane is placed beside it.
                workbench.open(pane: pane.id, title: Self.title(of: pane))
                pending = PendingSession(axis: axis)
            },
            continueOnAnotherModel: { askingForModel = ChoosingModel(id: pane.id) }
        )
    }

    /// Everything the palette offers, in the order it offers it.
    private func items(for kind: PaletteKind) -> [PaletteItem] {
        switch kind {
        case .sessions:
            return store.panes.map { pane in
                PaletteItem(
                    title: Self.title(of: pane),
                    detail: "\(pane.workspace) · \(pane.status.label)",
                    symbol: pane.agent == nil ? "terminal" : "sparkles"
                ) {
                    workbench.open(pane: pane.id, title: Self.title(of: pane))
                }
            }

        case .commands:
            var items: [PaletteItem] = [
                PaletteItem(
                    title: "New Session",
                    detail: "a project, an agent and a first prompt",
                    shortcut: "⌘N",
                    symbol: "plus"
                ) { pending = PendingSession(axis: nil) },
                PaletteItem(title: "Split Right", shortcut: "⌘D", symbol: "square.split.2x1") {
                    pending = PendingSession(axis: .horizontal)
                },
                PaletteItem(
                    title: "Split Down",
                    shortcut: "⇧⌘D",
                    symbol: "square.split.1x2"
                ) { pending = PendingSession(axis: .vertical) },
                PaletteItem(title: "Close Tab", shortcut: "⌘W", symbol: "xmark") {
                    guard let id = workbench.selected else { return }
                    for pane in workbench.closeTab(id) {
                        store.send(.detachPane(pane: pane))
                    }
                },
                PaletteItem(
                    title: "Usage and Night Shift",
                    detail: "limits, forecast, queue",
                    shortcut: "⌥⌘I",
                    symbol: "chart.line.uptrend.xyaxis"
                ) { inspector.toggle() },
                PaletteItem(
                    title: "Morning Report",
                    detail: "what otag did overnight",
                    shortcut: "⇧⌘R",
                    symbol: "sunrise"
                ) { store.askForMorningReport() },
            ]

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

            if let pane = workbench.focus.flatMap(store.pane), pane.agent != nil {
                items.append(PaletteItem(
                    title: "Hand Over to the Other Agent",
                    detail: Self.title(of: pane),
                    symbol: "arrow.left.arrow.right"
                ) { store.handOver(pane: pane.id, handoff: .agent(to: nil)) })
                items.append(PaletteItem(
                    title: "Continue on a Lighter Model",
                    detail: Self.title(of: pane),
                    symbol: "arrow.down.right.circle"
                ) { askingForModel = ChoosingModel(id: pane.id) })
                if pane.nightShift != nil {
                    items.append(PaletteItem(
                        title: "Stop the Night Shift",
                        detail: Self.title(of: pane),
                        symbol: "moon.zzz"
                    ) { store.send(.nightShift(pane: pane.id, plan: nil)) })
                }
            }

            return items
        }
    }

    /// The report is shown exactly while there is one.
    private var reportShown: Binding<Bool> {
        Binding(
            get: { store.morningReport != nil },
            set: { shown in if !shown { store.clearMorningReport() } }
        )
    }

    /// The sidebar's selection is the focused pane of the current tab, and
    /// choosing a session there opens or shows it.
    private var selection: Binding<PaneID?> {
        Binding(
            get: { workbench.focus },
            set: { pane in
                guard let pane else { return }
                let title = store.pane(pane).map(Self.title(of:)) ?? "session"
                workbench.open(pane: pane, title: title)
            }
        )
    }

    /// Put a freshly started session where the user asked for it.
    private func place(pane: PaneID, title: String, axis: SplitAxis?) {
        if let axis, workbench.current != nil {
            workbench.split(with: pane, axis: axis)
        } else {
            workbench.open(pane: pane, title: title)
        }
    }

    /// Keep each tab named after the pane it was opened for.
    private func retitleTabs(_ panes: [PaneInfo]) {
        for tab in workbench.tabs {
            guard let first = tab.panes.first,
                  let pane = panes.first(where: { $0.id == first })
            else { continue }
            workbench.rename(tab: tab.id, to: Self.title(of: pane))
        }
    }

    /// What a pane is called in a tab.
    static func title(of pane: PaneInfo) -> String {
        if !pane.title.isEmpty { return pane.title }
        return pane.agent?.label ?? "shell"
    }
}

/// What fills the detail side before anything is open.
struct EmptyDetailView: View {
    let state: ConnectionState
    let newSession: () -> Void

    var body: some View {
        VStack(spacing: 10) {
            switch state {
            case .connecting:
                ProgressView()
                Text("Connecting to the otag daemon…")
                    .foregroundStyle(.secondary)
            case .connected:
                Image(systemName: "rectangle.on.rectangle")
                    .font(.system(size: 34, weight: .light))
                    .foregroundStyle(.tertiary)
                Text("No session open")
                    .font(.title3)
                Text("Pick one on the left, or start one.")
                    .foregroundStyle(.secondary)
                Button("New Session…", action: newSession)
                    .padding(.top, 4)
            case let .failed(message):
                Image(systemName: "exclamationmark.triangle")
                    .font(.system(size: 34, weight: .light))
                    .foregroundStyle(.tertiary)
                Text("Not connected")
                    .font(.title3)
                Text(message)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 360)
            }
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
