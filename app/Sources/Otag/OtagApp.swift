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
    @State private var browsers = BrowserSessions()
    @State private var webViews = BrowserWebViews()
    @State private var pending: PendingSession?
    @State private var attention = Attention()
    @State private var palette = false
    @AppStorage("hasOnboarded") private var onboarded = false
    @AppStorage(Preference.theme) private var theme = TerminalThemeChoice.system.rawValue
    @Environment(\.colorScheme) private var appearance

    var body: some Scene {
        WindowGroup {
            MainView(
                store: store,
                workbench: workbench,
                names: names,
                browsers: browsers,
                webViews: webViews,
                pending: $pending,
                palette: $palette,
                onboarded: $onboarded
            )
            .task {
                attention.prepare()
                store.start()
            }
            // A program started in a pane is told which way round the
            // terminal is, so it can pick readable colours before it has
            // asked the pane anything. Sent on connecting and whenever the
            // theme changes; the daemon applies it to panes started after.
            .onChange(of: store.state) { _, state in
                if case .connected = state { tellTheDaemonAboutTheTheme() }
            }
            .onChange(of: theme) { _, _ in tellTheDaemonAboutTheTheme() }
            .onChange(of: appearance) { _, _ in tellTheDaemonAboutTheTheme() }
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
                Button(Strings.t("menu.newSession")) { pending = PendingSession(axis: nil) }
                    .keyboardShortcut("n", modifiers: .command)
            }
            // Cmd-K is for whoever already knows it is there. It is not on
            // the toolbar and not in the welcome: the menus and the sidebar
            // are the app, and a palette is a faster way to what they
            // already have.
            CommandGroup(after: .newItem) {
                Button(Strings.t("menu.palette")) { palette = true }
                    .keyboardShortcut("k", modifiers: .command)
            }
            CommandGroup(replacing: .help) {
                Button(Strings.t("menu.welcome")) { onboarded = false }
            }
            CommandMenu(Strings.t("menu.session")) {
                Button(Strings.t("menu.allowWaiting")) { answerFirst(.allow) }
                    .keyboardShortcut("y", modifiers: [.command, .shift])
                    .disabled(store.asks.isEmpty)
                Button(Strings.t("menu.denyWaiting")) { answerFirst(.deny) }
                    .keyboardShortcut("n", modifiers: [.command, .shift])
                    .disabled(store.asks.isEmpty)
                Divider()
                // Splitting means another session beside this one, and a
                // pane is the daemon's to create: the only honest way to get
                // one is to say what should run in it.
                Button(Strings.t("menu.splitRight")) {
                    pending = PendingSession(axis: .horizontal)
                }
                    .keyboardShortcut("d", modifiers: .command)
                Button(Strings.t("menu.splitDown")) {
                    pending = PendingSession(axis: .vertical)
                }
                    .keyboardShortcut("d", modifiers: [.command, .shift])
                Divider()
                Button(Strings.t("menu.closeSession")) { closeFocused() }
                    .keyboardShortcut("w", modifiers: .command)
                    .disabled(workbench.focus == nil)
                Button(Strings.t("menu.focusNext")) { workbench.stepFocus(by: 1) }
                    .keyboardShortcut("]", modifiers: .command)
                    .disabled(workbench.panes.count < 2)
                Button(Strings.t("menu.focusPrevious")) { workbench.stepFocus(by: -1) }
                    .keyboardShortcut("[", modifiers: .command)
                    .disabled(workbench.panes.count < 2)
                Divider()
                Button(handOffTitle) { handOff() }
                    .disabled(focusedPane?.agent == nil)
                Button(Strings.t("menu.report")) { store.askForMorningReport() }
                    .keyboardShortcut("r", modifiers: [.command, .shift])
            }
            // ⌘1…⌘9 go to the first nine sessions in the sidebar's order.
            // The sidebar shows each row's number, so the shortcut teaches
            // itself rather than needing a list to memorise.
            CommandMenu(Strings.t("menu.go")) {
                ForEach(1 ... 9, id: \.self) { number in
                    Button(sessionName(number)) { go(to: number) }
                        .keyboardShortcut(KeyEquivalent(Character("\(number)")), modifiers: .command)
                        .disabled(orderedSessions.count < number)
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

    /// Tell the daemon whether the panes are being drawn on dark paper.
    private func tellTheDaemonAboutTheTheme() {
        let resolved = TerminalThemeChoice.stored(theme).resolved(dark: appearance == .dark)
        store.send(.terminalAppearance(dark: resolved.isDark))
    }

    /// The session the window is showing.
    private var focusedPane: PaneInfo? {
        workbench.focus.flatMap(store.pane)
    }

    private var handOffTitle: String {
        switch focusedPane?.agent {
        case .claude: return Strings.t("switch.handoff.codex")
        case .codex: return Strings.t("switch.handoff.claude")
        case nil: return Strings.t("switch.handoff.none")
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
        if browsers.session(pane) != nil {
            browsers.close(pane)
            workbench.close(pane: pane)
            return
        }
        store.send(.detachPane(pane: pane))
        workbench.close(pane: pane)
    }

    /// Show the nth session in the sidebar's order.
    private func go(to number: Int) {
        let ordered = orderedSessions
        guard ordered.indices.contains(number - 1) else { return }
        workbench.select(pane: ordered[number - 1])
    }

    private var orderedSessions: [PaneID] {
        store.ordered + browsers.sessions.map(\.id)
    }

    /// What `⌘n` would go to, so the menu reads as sessions rather than
    /// numbers.
    private func sessionName(_ number: Int) -> String {
        let ordered = orderedSessions
        guard ordered.indices.contains(number - 1) else { return Strings.t("menu.go.nth", number) }
        let id = ordered[number - 1]
        if let browser = browsers.session(id) { return browser.url.host ?? Strings.t("browser.title") }
        guard let pane = store.pane(id) else { return Strings.t("menu.go.nth", number) }
        return names.name(of: pane)
    }
}

/// The window: the sessions on the left, the selected one on the right.
struct MainView: View {
    let store: SessionStore
    let workbench: Workbench
    let names: SessionNames
    let browsers: BrowserSessions
    let webViews: BrowserWebViews
    @Binding var pending: PendingSession?
    @Binding var palette: Bool
    @Binding var onboarded: Bool

    /// What the window is asking about one session, when it is asking.
    @State private var prompt: SessionPrompt?
    /// Whether this window is visible; terminal streams follow it.
    @State private var windowVisible = true
    /// Which toolbar popover is open.
    @State private var guarding = false
    @State private var switching = false

    var body: some View {
        NavigationSplitView {
            SidebarView(
                store: store,
                names: names,
                workbench: workbench,
                browsers: browsers,
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
                          (store.pane(dropped.pane) != nil || browsers.session(dropped.pane) != nil) else { return false }
                    workbench.select(pane: dropped.pane)
                    return true
                }
            } else {
                PanesView(store: store, names: names, workbench: workbench, browsers: browsers, webViews: webViews)
            }
        }
        .environment(\.windowVisible, windowVisible)
        .background {
            WindowVisibility { visible, anyVisible in
                windowVisible = visible
                store.setUIActive(anyVisible)
            }
            .frame(width: 0, height: 0)
        }
        .toolbar { toolbar }
        .sheet(item: $pending) { request in
            NewSessionSheet(
                socket: store.socket,
                suggestedProject: store.suggestedProject(near: workbench.focus),
                startBrowser: { path in
                    let project = store.panes.first { $0.cwd == path }?.workspace
                        ?? (path.isEmpty ? "" : URL(fileURLWithPath: path).lastPathComponent)
                    place(pane: browsers.create(project: project), axis: request.axis)
                }
            , started: { pane in
                place(pane: pane, axis: request.axis)
            }, teamStarted: { panes in
                workbench.arrange(panes, keeping: panes.first)
            })
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
            workbench.restore(alive: panes.map(\.id) + browsers.sessions.map(\.id))

            // A pane the daemon has let go of cannot stay on screen.
            let alive = Set(panes.map(\.id) + browsers.sessions.map(\.id))
            for pane in workbench.panes where !alive.contains(pane) {
                workbench.close(pane: pane)
            }
            // The daemon's list is what the session numbering counts along,
            // and what a rename of a session that is over is pruned against.
            names.took(panes)
            workbench.save()
        }
        .onChange(of: store.hasLoadedPanes) { _, loaded in
            if loaded {
                workbench.restore(alive: store.panes.map(\.id) + browsers.sessions.map(\.id))
            }
        }
        .onChange(of: browsers.sessions.map(\.id)) { _, ids in
            let alive = Set(ids)
            webViews.retain(alive)
            for pane in workbench.panes where store.pane(pane) == nil && !alive.contains(pane) {
                workbench.close(pane: pane)
            }
            workbench.save()
        }
        .onChange(of: workbench.root) { _, _ in workbench.save() }
        .onChange(of: workbench.focus) { _, _ in workbench.save() }
    }

    /// Five buttons, one job each, in one style.
    ///
    /// Arrange acts at once; the guard, switching and the language open
    /// something; `+` starts a session. A button that would do nothing is
    /// disabled rather than hidden, because a toolbar that changes shape is
    /// a toolbar nobody learns.
    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            if let team = focused?.team {
                Button {
                    store.send(.teamMode(team: team.id, automatic: !team.automatic))
                } label: {
                    Label(Strings.t(team.automatic ? "team.auto" : "team.manual"), systemImage: team.automatic ? "play.circle" : "hand.tap")
                }
                .help(Strings.t("team.mode.help"))
            }
        }
        ToolbarItem(placement: .primaryAction) {
            ToolbarIcon(
                symbol: workbench.isArranged ? "rectangle" : "rectangle.split.2x2",
                title: Strings.t("toolbar.arrange.title"),
                label: Strings.t(workbench.isArranged
                    ? "toolbar.arrange.undo" : "toolbar.arrange"),
                enabled: workbench.isArranged || arrangementIDs.count > 1,
                action: arrange
            )
        }
        ToolbarItem(placement: .primaryAction) {
            ToolbarPopoverIcon(
                symbol: anyGuarded ? "shield.fill" : "shield",
                title: Strings.t("toolbar.guard.title"),
                label: Strings.t("toolbar.guard"),
                tinted: anyGuarded,
                shown: $guarding
            ) {
                GuardPopover(store: store, names: names, workbench: workbench)
            }
        }
        ToolbarItem(placement: .primaryAction) {
            ToolbarPopoverIcon(
                symbol: "arrow.left.arrow.right",
                title: Strings.t("toolbar.switch.title"),
                label: Strings.t("toolbar.switch"),
                enabled: focused?.agent != nil,
                shown: $switching
            ) {
                if let pane = focused {
                    SwitchAgentPopover(actions: actions(for: pane))
                }
            }
        }
        ToolbarItem(placement: .primaryAction) {
            LanguageMenu()
        }
        ToolbarItem(placement: .primaryAction) {
            Menu {
                Button(Strings.t("menu.newSession")) { pending = PendingSession(axis: nil) }
                Button(Strings.t("browser.title")) { newBrowser() }
            } label: {
                Label(Strings.t("toolbar.new.title"), systemImage: "plus")
                    .labelStyle(.titleAndIcon)
                    .foregroundStyle(.primary)
            }
            .menuStyle(.button)
            .labelStyle(.titleAndIcon)
            .menuIndicator(.hidden)
            .chromeButtonStyle()
            .controlSize(.regular)
            .fixedSize()
            .help(Strings.t("toolbar.new"))
        }
    }

    /// The sessions of the project the selected session belongs to, most
    /// recently used first.
    ///
    /// Arrange lays out a *project*, not the whole daemon: four sessions in
    /// one checkout is the case it is for.
    private var inThisProject: [PaneInfo] {
        guard let project = focused?.workspace ?? workbench.focus.flatMap(browsers.session)?.project else { return [] }
        let recent = workbench.recentlyUsed
        return store.panes
            .filter { $0.workspace == project }
            .sorted { one, two in
                // Most recently used first; the ones the window has never
                // shown keep the daemon's order behind them.
                let first = recent.firstIndex(of: one.id) ?? Int.max
                let second = recent.firstIndex(of: two.id) ?? Int.max
                return first == second ? one.id < two.id : first < second
            }
    }

    private var arrangementIDs: [PaneID] {
        let project = focused?.workspace ?? workbench.focus.flatMap(browsers.session)?.project ?? ""
        let ids = inThisProject.map(\.id) + browsers.sessions.filter { $0.project == project }.map(\.id)
        let recent = workbench.recentlyUsed
        return ids.sorted {
            (recent.firstIndex(of: $0) ?? Int.max) < (recent.firstIndex(of: $1) ?? Int.max)
        }
    }

    private func newBrowser() {
        let project = focused?.workspace ?? workbench.focus.flatMap(browsers.session)?.project ?? ""
        place(pane: browsers.create(project: project), axis: nil)
    }

    private func arrange() {
        workbench.arrange(arrangementIDs, keeping: workbench.focus)
    }

    private var anyGuarded: Bool {
        store.panes.contains { $0.guardPlan != nil }
    }

    private func actions(for pane: PaneInfo) -> SessionActions {
        SessionActions(store: store, workbench: workbench, names: names, pane: pane)
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
                detail: "\(pane.workspace) · \(pane.status.localizedPrecise)",
                symbol: pane.agent == nil ? "terminal" : "sparkles"
            ) {
                workbench.select(pane: pane.id)
            }
        }

        items.append(PaletteItem(
            title: Strings.t("palette.new"),
            detail: Strings.t("palette.new.detail"),
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
                    title: Strings.t("palette.switch"),
                    detail: actions.name,
                    symbol: "arrow.left.arrow.right"
                ) { switching = true })
                items.append(PaletteItem(
                    title: Strings.t("menu.nightshift"),
                    detail: actions.name,
                    symbol: "moon.stars"
                ) { prompt = SessionPrompt(pane: pane.id, kind: .nightShift) })
            }
            items.append(PaletteItem(
                title: Strings.t("menu.rename"),
                detail: actions.name,
                symbol: "pencil"
            ) { prompt = SessionPrompt(pane: pane.id, kind: .rename) })
            items.append(PaletteItem(
                title: Strings.t("menu.close"),
                detail: actions.name,
                shortcut: "⌘W",
                symbol: "xmark"
            ) { actions.close() })
        }

        items.append(PaletteItem(
            title: Strings.t("palette.report"),
            detail: Strings.t("palette.report.detail"),
            shortcut: "⇧⌘R",
            symbol: "sunrise"
        ) { store.askForMorningReport() })

        for ask in store.asks {
            items.append(PaletteItem(
                title: Strings.t("menubar.allow", ask.summary),
                detail: ask.agent?.label ?? "",
                symbol: "checkmark.circle"
            ) { store.answer(ask, with: .allow) })
            items.append(PaletteItem(
                title: Strings.t("menubar.deny", ask.summary),
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
