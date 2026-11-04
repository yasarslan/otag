import OtagKit
import SwiftUI

/// The detail side of the window: a strip of tabs, and the panes of the one
/// that is showing.
struct TabsView: View {
    let store: SessionStore
    let workbench: Workbench
    /// A session the user has asked for and the daemon has not started yet,
    /// which is what a split is: the daemon owns the panes, so the only
    /// honest way to get one is to say what should run in it.
    @Binding var pending: PendingSession?
    /// The session whose model the user is choosing, if any.
    @Binding var askingForModel: ChoosingModel?

    var body: some View {
        VStack(spacing: 0) {
            if workbench.tabs.count > 1 {
                TabStrip(store: store, workbench: workbench)
                Divider()
            }
            if let tab = workbench.current {
                PaneTreeView(
                    node: tab.root,
                    tab: tab,
                    store: store,
                    workbench: workbench,
                    pending: $pending,
                    askingForModel: $askingForModel
                )
                .id(tab.id)
            }
        }
    }
}

/// One row of tabs, as flat and quiet as the rest of the window.
private struct TabStrip: View {
    let store: SessionStore
    let workbench: Workbench

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 0) {
                ForEach(workbench.tabs) { tab in
                    TabButton(
                        tab: tab,
                        selected: tab.id == workbench.selected,
                        status: status(of: tab),
                        select: { workbench.selected = tab.id },
                        close: { close(tab.id) }
                    )
                    Divider().frame(height: 16)
                }
            }
            .padding(.horizontal, 4)
        }
        .frame(height: 30)
    }

    /// What a tab is doing: the status of whichever of its panes most wants
    /// the user, which is the same rule the sidebar uses.
    private func status(of tab: TerminalTab) -> PaneStatus {
        tab.panes.compactMap { store.pane($0)?.status }.max() ?? .idle
    }

    private func close(_ id: UUID) {
        for pane in workbench.closeTab(id) {
            store.send(.detachPane(pane: pane))
        }
    }
}

/// One tab.
private struct TabButton: View {
    let tab: TerminalTab
    let selected: Bool
    let status: PaneStatus
    let select: () -> Void
    let close: () -> Void

    @State private var hovering = false

    var body: some View {
        HStack(spacing: 5) {
            if status.wantsAttention {
                Circle()
                    .fill(.tint)
                    .frame(width: 6, height: 6)
            }
            Text(tab.title)
                .font(.system(size: 12))
                .lineLimit(1)
            Button(action: close) {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .bold))
            }
            .buttonStyle(.plain)
            .opacity(hovering || selected ? 0.6 : 0)
            .help("Close this tab")
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background(selected ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear),
                    in: RoundedRectangle(cornerRadius: 6))
        .contentShape(Rectangle())
        .onTapGesture(perform: select)
        .onHover { hovering = $0 }
    }
}

/// The panes of one tab, in the arrangement the workbench holds.
///
/// Recursive, like the tree it draws: a split gives each child its fraction
/// of the space and a divider that can be dragged between them.
private struct PaneTreeView: View {
    let node: PaneNode
    let tab: TerminalTab
    let store: SessionStore
    let workbench: Workbench
    @Binding var pending: PendingSession?
    @Binding var askingForModel: ChoosingModel?

    var body: some View {
        switch node {
        case let .leaf(pane):
            PaneCell(
                pane: pane,
                tab: tab,
                store: store,
                workbench: workbench,
                pending: $pending,
                askingForModel: $askingForModel
            )

        case let .split(axis, children, fractions):
            GeometryReader { geometry in
                let along = axis == .horizontal ? geometry.size.width : geometry.size.height
                let dividers = Double(max(children.count - 1, 0)) * Self.dividerWidth
                let usable = max(along - dividers, 1)

                let content = ForEach(Array(children.enumerated()), id: \.element.id) { index, child in
                    PaneTreeView(
                        node: child,
                        tab: tab,
                        store: store,
                        workbench: workbench,
                        pending: $pending,
                        askingForModel: $askingForModel
                    )
                        .frame(
                            width: axis == .horizontal
                                ? usable * share(fractions, index) : nil,
                            height: axis == .vertical
                                ? usable * share(fractions, index) : nil
                        )
                    if index < children.count - 1 {
                        DividerHandle(axis: axis) { delta in
                            workbench.move(divider: index, of: node.id, by: delta / usable)
                        }
                    }
                }

                if axis == .horizontal {
                    HStack(spacing: 0) { content }
                } else {
                    VStack(spacing: 0) { content }
                }
            }
        }
    }

    /// How wide a divider is, and how much room there is to grab it.
    private static let dividerWidth: Double = 5

    private func share(_ fractions: [Double], _ index: Int) -> Double {
        guard fractions.indices.contains(index) else { return 0 }
        let total = fractions.reduce(0, +)
        return total > 0 ? fractions[index] / total : 0
    }
}

/// A draggable line between two panes.
private struct DividerHandle: View {
    let axis: SplitAxis
    let move: (Double) -> Void

    var body: some View {
        Rectangle()
            .fill(.separator)
            .frame(
                width: axis == .horizontal ? 5 : nil,
                height: axis == .vertical ? 5 : nil
            )
            .contentShape(Rectangle())
            .onHover { inside in
                if inside {
                    (axis == .horizontal ? NSCursor.resizeLeftRight : NSCursor.resizeUpDown).push()
                } else {
                    NSCursor.pop()
                }
            }
            .gesture(
                DragGesture(minimumDistance: 1)
                    .onChanged { value in
                        // The gesture reports the whole translation each
                        // time, so only what is new is passed on.
                        let amount = axis == .horizontal
                            ? value.translation.width : value.translation.height
                        move(Double(amount) - carried)
                        carried = Double(amount)
                    }
                    .onEnded { _ in carried = 0 }
            )
    }

    @State private var carried: Double = 0
}

/// One pane in a tab: the terminal, with a thin ring when it has the
/// keyboard and more than one pane is sharing the tab.
private struct PaneCell: View {
    let pane: PaneID
    let tab: TerminalTab
    let store: SessionStore
    let workbench: Workbench
    @Binding var pending: PendingSession?
    @Binding var askingForModel: ChoosingModel?

    @AppStorage(Preference.dimIdle) private var dimIdle = true
    /// Why this pane cannot be followed any more, once that has happened.
    @State private var gone: String?

    var body: some View {
        VStack(spacing: 0) {
            // The frequent actions, where the session is, rather than only
            // behind a palette the user has to know about.
            if let info = store.pane(pane) {
                SessionHeader(
                    commands: commands(for: info),
                    focused: tab.focus == pane
                )
            }
            terminal
        }
        .onTapGesture { workbench.focus(pane: pane) }
        // A session doing nothing, with the keyboard somewhere else, fades
        // slightly into the background. Nothing moves and nothing blinks;
        // the eye simply goes to what is working.
        .opacity(dimmed ? 0.72 : 1)
    }

    /// Everything there is to do with this session.
    private func commands(for info: PaneInfo) -> SessionCommands {
        SessionCommands(
            store: store,
            workbench: workbench,
            pane: info,
            split: { axis in
                // Split from *this* session, whichever one has the keyboard.
                workbench.focus(pane: pane)
                pending = PendingSession(axis: axis)
            },
            continueOnAnotherModel: { askingForModel = ChoosingModel(id: pane) }
        )
    }

    private var terminal: some View {
        TerminalPaneView(
            pane: pane,
            socket: store.socket,
            focused: tab.focus == pane && tab.panes.count > 1,
            onEnded: { _ in workbench.close(pane: pane) },
            onGone: { message in gone = message },
            onFocus: { workbench.focus(pane: pane) }
        )
        .overlay {
            if let gone {
                // The daemon is running and has no such pane: the session is
                // over, and what is on the terminal is the last thing it
                // ever showed. Saying so beats a view that quietly does
                // nothing when it is typed into.
                PaneGoneOverlay(reason: gone) { workbench.close(pane: pane) }
            }
        }
        .overlay {
            if tab.panes.count > 1, tab.focus == pane {
                RoundedRectangle(cornerRadius: 3)
                    .strokeBorder(.tint.opacity(0.5), lineWidth: 1)
            }
        }
    }

    /// Whether this pane should be drawn dimmer.
    private var dimmed: Bool {
        guard dimIdle, tab.panes.count > 1, tab.focus != pane else { return false }
        let status = store.pane(pane)?.status ?? .idle
        return status == .idle || status == .done
    }
}

/// Said over a pane the daemon no longer has.
///
/// Not a dialog and not an alert: the terminal stays readable underneath,
/// because its last screen is often the thing the user wants to look at.
private struct PaneGoneOverlay: View {
    let reason: String
    let close: () -> Void

    var body: some View {
        VStack(spacing: 8) {
            Spacer()
            VStack(spacing: 4) {
                Label("This session is no longer running", systemImage: "xmark.circle")
                    .font(.callout)
                Text(reason)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                Button("Close", action: close)
                    .padding(.top, 4)
            }
            .padding(14)
            .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 8))
            .padding(12)
        }
    }
}
