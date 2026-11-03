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

    var body: some Scene {
        WindowGroup {
            MainView(store: store, workbench: workbench, pending: $pending)
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
            }
        }

        // Visible with the window closed, which is how an agent that needs
        // permission is found without going looking for the app.
        MenuBarExtra {
            MenuBarView(store: store)
        } label: {
            MenuBarLabel(store: store)
        }
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
    @State private var inspector = false

    var body: some View {
        NavigationSplitView {
            SidebarView(store: store, selection: selection)
                .navigationSplitViewColumnWidth(min: 220, ideal: 260, max: 380)
        } detail: {
            if workbench.tabs.isEmpty {
                EmptyDetailView(state: store.state) {
                    pending = PendingSession(axis: nil)
                }
            } else {
                TabsView(store: store, workbench: workbench)
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
            NewSessionSheet(socket: store.socket) { pane, title in
                place(pane: pane, title: title, axis: request.axis)
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
