import AppKit
import OtagKit
import SwiftUI

/// The main area: the sessions the window is showing, each under its own
/// header bar.
///
/// One session fills it. Two or three divide it, in the arrangement the
/// workbench holds, with a divider between them that can be dragged.
struct PanesView: View {
    let store: SessionStore
    let names: SessionNames
    let workbench: Workbench

    var body: some View {
        Group {
            if let root = workbench.root {
                PaneTree(node: root, store: store, names: names, workbench: workbench)
                    // Drag a session out of the sidebar and onto a side to
                    // put it there. Nothing is drawn until something is over
                    // them.
                    .overlay { DropEdges(workbench: workbench, store: store) }
            }
        }
    }
}

/// The panes in the arrangement the workbench holds.
///
/// Recursive, like the tree it draws: a split gives each child its fraction
/// of the space and a divider that can be dragged between them.
private struct PaneTree: View {
    let node: PaneNode
    let store: SessionStore
    let names: SessionNames
    let workbench: Workbench

    var body: some View {
        switch node {
        case let .leaf(pane):
            // Keyed by the session it is showing, and this is load-bearing.
            // A terminal is an `NSViewRepresentable` whose coordinator holds
            // one pane's byte stream, built once in `makeCoordinator`. When
            // the showing session changed from one leaf to another, SwiftUI
            // saw the same view in the same place, reused that coordinator,
            // and the window went on showing the *old* session's output —
            // selecting a session in the sidebar appeared to do nothing.
            // The identity makes the old view go and a new one arrive, which
            // is what attaching to a different pane actually is.
            PaneCell(pane: pane, store: store, names: names, workbench: workbench)
                .id(pane)

        case let .split(axis, children, fractions):
            GeometryReader { geometry in
                let along = axis == .horizontal ? geometry.size.width : geometry.size.height
                let dividers = Double(max(children.count - 1, 0)) * Self.dividerWidth
                let usable = max(along - dividers, 1)

                let content = ForEach(Array(children.enumerated()), id: \.element.id) { index, child in
                    PaneTree(
                        node: child,
                        store: store,
                        names: names,
                        workbench: workbench
                    )
                    .frame(
                        width: axis == .horizontal ? usable * share(fractions, index) : nil,
                        height: axis == .vertical ? usable * share(fractions, index) : nil
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

    @State private var carried: Double = 0

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
}

/// One session: its header bar, and its terminal under it.
private struct PaneCell: View {
    let pane: PaneID
    let store: SessionStore
    let names: SessionNames
    let workbench: Workbench

    @AppStorage(Preference.dimIdle) private var dimIdle = true
    /// Why this pane cannot be followed any more, once that has happened.
    @State private var gone: String?

    var body: some View {
        VStack(spacing: 0) {
            if let info = store.pane(pane) {
                SessionHeaderBar(
                    actions: SessionActions(
                        store: store,
                        workbench: workbench,
                        names: names,
                        pane: info
                    ),
                    focused: workbench.focus == pane,
                    sharing: workbench.panes.count > 1
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

    private var terminal: some View {
        TerminalPaneView(
            pane: pane,
            socket: store.socket,
            focused: workbench.focus == pane && workbench.panes.count > 1,
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
            if workbench.panes.count > 1, workbench.focus == pane {
                RoundedRectangle(cornerRadius: 3)
                    .strokeBorder(.tint.opacity(0.5), lineWidth: 1)
            }
        }
    }

    /// Whether this pane should be drawn dimmer.
    private var dimmed: Bool {
        guard dimIdle, workbench.panes.count > 1, workbench.focus != pane else { return false }
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
        VStack(spacing: Chrome.gap) {
            Spacer()
            VStack(spacing: 4) {
                Label(Strings.t("pane.gone"), systemImage: "xmark.circle")
                    .font(.callout)
                Text(reason)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                Button(Strings.t("pane.gone.close"), action: close)
                    .padding(.top, 4)
            }
            .padding(14)
            .cardBackground()
            .padding(12)
        }
    }
}
