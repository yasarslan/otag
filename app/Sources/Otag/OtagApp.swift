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
    @AppStorage(Preference.theme) private var theme = TerminalThemeChoice.glassDark.rawValue

    var body: some Scene {
        WindowGroup("Otag") {
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
        .windowToolbarStyle(.unified(showsTitle: true))
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
        store.send(.terminalAppearance(dark: TerminalProfile.theme(for: theme).isDark))
    }

    /// The session the window is showing.
    private var focusedPane: PaneInfo? {
        workbench.focus.flatMap(store.pane)
    }

    private var handOffTitle: String {
        switch focusedPane?.agent {
        case .claude: return Strings.t("switch.handoff.codex")
        case .codex, .deepseek: return Strings.t("switch.handoff.claude")
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
        store.close(.pane(pane))
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
    @State private var sidebarShown = true
    /// Which toolbar popover is open.
    @State private var guarding = false
    @State private var switching = false
    @State private var apiProviderShown = false

    var body: some View {
        HSplitView {
            if sidebarShown {
                SidebarView(
                    store: store,
                    names: names,
                    workbench: workbench,
                    browsers: browsers,
                    selection: selection,
                    ask: { pane, kind in prompt = SessionPrompt(pane: pane, kind: kind) }
                )
                .frame(minWidth: 220, idealWidth: 260, maxWidth: 380, maxHeight: .infinity)
                .background(Chrome.sidebar)
            }
            Group {
                if workbench.root == nil {
                    EmptyStateView(state: store.state) {
                        pending = PendingSession(axis: nil)
                    }
                    .background(Chrome.header)
                    // With nothing showing, a session dragged anywhere here opens it.
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
            .frame(idealWidth: 840, maxWidth: .infinity, maxHeight: .infinity)
            .layoutPriority(1)
        }
        .environment(\.windowVisible, windowVisible)
        .background(Chrome.header)
        .workspaceBackground()
        .background {
            WindowVisibility { visible, anyVisible in
                windowVisible = visible
                store.setUIActive(anyVisible)
            }
            .frame(width: 0, height: 0)
        }
        .toolbar { toolbar }
        .alert(
            Strings.t("close.confirm.title"),
            isPresented: Binding(
                get: { store.closeConfirmation != nil },
                set: { if !$0 { store.resolveClose() } }
            ),
            presenting: store.closeConfirmation
        ) { target in
            Button(Strings.t(target.titleKey), role: .destructive) {
                store.resolveClose(confirming: target)
            }
            Button(Strings.t("sheet.cancel"), role: .cancel) {}
        } message: { target in
            Text(Strings.t(target.confirmationKey))
        }
        .alert(
            Strings.t("team.advanceWarning.title"),
            isPresented: Binding(
                get: { AppFeatures.teamMode && store.teamAdvanceWarning != nil },
                set: { if !$0 { store.teamAdvanceWarning = nil } }
            ),
            presenting: store.teamAdvanceWarning
        ) { warning in
            Button(Strings.t("team.advanceWarning.continue")) {
                store.send(.teamAdvance(
                    pane: warning.pane, revision: warning.revision,
                    back: warning.back, confirmation: warning.confirmation
                ))
            }
            Button(Strings.t("sheet.cancel"), role: .cancel) {}
        } message: { warning in
            Text(warning.result == "MISSING"
                ? Strings.t("team.advanceWarning.missing")
                : Strings.t("team.advanceWarning.detail", warning.result))
        }
        .sheet(item: $pending) { request in
            NewSessionSheet(
                socket: store.socket,
                suggestedProject: store.suggestedProject(near: workbench.focus),
                provider: store.apiProvider,
                startBrowser: { path in
                    let project = store.panes.first { $0.cwd == path }?.workspace
                        ?? (path.isEmpty ? "" : URL(fileURLWithPath: path).lastPathComponent)
                    place(pane: browsers.create(project: project), axis: request.axis)
                }
            , started: { pane in
                place(pane: pane, axis: request.axis)
            }, teamStarted: { panes in
                if let first = panes.first { workbench.showTeam(panes, focused: first) }
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
            showFocusedTeam()
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
        .onChange(of: workbench.root) { _, _ in
            showFocusedTeam()
            workbench.save()
        }
        .onChange(of: workbench.focus) { _, _ in
            showFocusedTeam()
            workbench.save()
        }
    }

    /// Toolbar actions, one job each, in one style.
    ///
    /// Arrange acts at once; the guard, switching and API configuration open
    /// a popover; `+` starts a session. A button that would do nothing is
    /// disabled rather than hidden, because a toolbar that changes shape is
    /// a toolbar nobody learns.
    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            Button {
                sidebarShown.toggle()
            } label: {
                Label(Strings.t(sidebarShown ? "sidebar.hide" : "sidebar.show"), systemImage: "sidebar.left")
            }
            .labelStyle(.iconOnly)
            .keyboardShortcut("s", modifiers: [.command, .control])
        }
        if let team = focusedTeam {
            ToolbarItem(placement: .primaryAction) {
                HStack(spacing: 12) {
                    teamAdvanceButton(team.id)
                    Toggle(Strings.t("team.auto"), isOn: Binding(
                        get: { team.automatic },
                        set: { store.send(.teamMode(team: team.id, automatic: $0)) }
                    ))
                    .toggleStyle(.checkbox)
                    .controlSize(.regular)
                    .fixedSize()
                }
            }
        }
        ToolbarItem(placement: .primaryAction) {
            ToolbarIcon(
                symbol: workbench.isArranged ? "rectangle" : "rectangle.split.2x2",
                title: Strings.t("toolbar.arrange.title"),
                label: Strings.t(workbench.isArranged
                    ? "toolbar.arrange.undo" : "toolbar.arrange"),
                enabled: focusedTeam == nil && (workbench.isArranged || arrangementIDs.count > 1),
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
            ToolbarPopoverIcon(symbol: "key", title: Strings.t("api.title"), label: Strings.t("api.title"), shown: $apiProviderShown) {
                APIProviderPopover(store: store)
            }
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
        guard focusedTeam == nil else { return }
        workbench.arrange(arrangementIDs, keeping: workbench.focus)
    }

    private var focusedTeam: TeamMembership? {
        AppFeatures.teamMode ? focused?.team : nil
    }

    private func showFocusedTeam() {
        guard let team = focusedTeam, let pane = focused else { return }
        let members = TeamMembership.members(of: team.id, in: store.panes).map(\.id)
        workbench.showTeam(members, focused: pane.id)
    }

    private func teamAdvanceButton(_ team: UInt64) -> some View {
        let current = TeamMembership.members(of: team, in: store.panes)
            .first { $0.team?.current == true }
        let role = current?.team
        let title = if role?.waiting == true {
            Strings.t("team.startStep")
        } else if let next = role?.nextRole {
            Strings.t("team.sendTo", next)
        } else if role != nil {
            Strings.t("team.finish")
        } else {
            Strings.t("team.finished")
        }
        return Button(title) {
            guard let current, let role else { return }
            store.send(.teamAdvance(pane: current.id, revision: role.revision, back: false))
        }
        .buttonStyle(.borderedProminent)
        .tint(.secondary)
        .disabled(role?.canAdvance != true)
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
