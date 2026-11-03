import OtagKit
import SwiftUI

/// Otag: a native window onto the otag daemon.
///
/// The daemon owns the panes, the agents and everything known about them;
/// this app connects to it, shows what it reports, and sends what the user
/// does. Quitting the app leaves every agent running.
@main
struct OtagApp: App {
    @State private var store = SessionStore()

    var body: some Scene {
        WindowGroup {
            MainView(store: store)
                .task { store.start() }
        }
        .defaultSize(width: 1_100, height: 700)
        .windowToolbarStyle(.unified)
    }
}

/// The window: sessions on the left, the selected session on the right.
struct MainView: View {
    let store: SessionStore
    @State private var selection: PaneID?

    var body: some View {
        NavigationSplitView {
            SidebarView(store: store, selection: $selection)
                .navigationSplitViewColumnWidth(min: 220, ideal: 260, max: 380)
        } detail: {
            if let selection, let pane = store.pane(selection) {
                SessionView(pane: pane)
            } else {
                EmptyDetailView(state: store.state)
            }
        }
        .onChange(of: store.panes) { _, panes in
            // A session that has gone cannot stay selected, and a window
            // with nothing selected should show the first session there is.
            if let selection, panes.contains(where: { $0.id == selection }) {
                return
            }
            selection = panes.first?.id
        }
    }
}

/// What fills the detail side before a session is chosen.
struct EmptyDetailView: View {
    let state: ConnectionState

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
                Text("No session selected")
                    .font(.title3)
                Text("Pick a session on the left.")
                    .foregroundStyle(.secondary)
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
