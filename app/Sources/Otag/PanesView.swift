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
    let browsers: BrowserSessions
    let webViews: BrowserWebViews

    var body: some View {
        Group {
            if let root = workbench.root {
                PaneTree(node: root, store: store, names: names, workbench: workbench, browsers: browsers, webViews: webViews)
                    // Drag a session out of the sidebar and onto a side to
                    // put it there. Nothing is drawn until something is over
                    // them.
                    .overlay { DropEdges(workbench: workbench, store: store, browsers: browsers) }
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
    let browsers: BrowserSessions
    let webViews: BrowserWebViews
    @Environment(\.pixelLength) private var pixelLength

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
            PaneCell(pane: pane, store: store, names: names, workbench: workbench, browsers: browsers, webViews: webViews)
                .id(pane)

        case let .split(axis, children, fractions):
            GeometryReader { geometry in
                let along = axis == .horizontal ? geometry.size.width : geometry.size.height
                let dividers = Double(max(children.count - 1, 0)) * pixelLength
                let usable = max(along - dividers, 1)

                let content = ForEach(Array(children.enumerated()), id: \.element.id) { index, child in
                    PaneTree(
                        node: child,
                        store: store,
                        names: names,
                        workbench: workbench,
                        browsers: browsers,
                        webViews: webViews
                    )
                    .frame(
                        width: axis == .horizontal ? usable * share(fractions, index) : nil,
                        height: axis == .vertical ? usable * share(fractions, index) : nil
                    )
                    if index < children.count - 1 {
                        DividerHandle(axis: axis) { delta in
                            workbench.move(divider: index, of: node.id, by: delta / usable)
                        }
                        .zIndex(1)
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

    private func share(_ fractions: [Double], _ index: Int) -> Double {
        guard fractions.indices.contains(index) else { return 0 }
        let total = fractions.reduce(0, +)
        return total > 0 ? fractions[index] / total : 0
    }
}

/// A draggable line between two panes.
private struct DividerHandle: View {
    private static let hitWidth: CGFloat = 5
    let axis: SplitAxis
    let move: (Double) -> Void

    @Environment(\.pixelLength) private var pixelLength
    @State private var carried: Double = 0

    var body: some View {
        Chrome.splitHairline
            .frame(
                width: axis == .horizontal ? pixelLength : nil,
                height: axis == .vertical ? pixelLength : nil
            )
            .overlay { hitTarget }
    }

    private var hitTarget: some View {
        Color.clear
            .frame(
                width: axis == .horizontal ? Self.hitWidth : nil,
                height: axis == .vertical ? Self.hitWidth : nil
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
    let browsers: BrowserSessions
    let webViews: BrowserWebViews

    @AppStorage(Preference.dimIdle) private var dimIdle = true
    @Environment(\.pixelLength) private var pixelLength
    /// Why this pane cannot be followed any more, once that has happened.
    @State private var gone: String?

    var body: some View {
        VStack(spacing: 0) {
            if let browser = browsers.session(pane) {
                BrowserPaneView(page: webViews.page(for: browser, sessions: browsers)) {
                    browsers.close(pane)
                    workbench.close(pane: pane)
                }
            } else {
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
        }
        .onTapGesture { workbench.focus(pane: pane) }
        .overlay {
            if let borderColour {
                Rectangle()
                    .strokeBorder(borderColour, lineWidth: pixelLength)
                    .allowsHitTesting(false)
            }
        }
    }

    private var borderColour: Color? {
        if store.pane(pane)?.status.wantsAttention == true || store.asks.contains(where: { $0.pane == pane }) {
            return Chrome.attention
        }
        return workbench.focus == pane ? Chrome.paneFocus : nil
    }

    private var terminal: some View {
        TerminalPaneView(
            pane: pane,
            socket: store.socket,
            focused: workbench.focus == pane && workbench.panes.count > 1,
            dimmed: dimmed,
            onEnded: { _ in if store.pane(pane)?.team == nil { workbench.close(pane: pane) } },
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
    }

    /// Whether this pane should be drawn dimmer.
    private var dimmed: Bool {
        guard browsers.session(pane) == nil, dimIdle, workbench.panes.count > 1, workbench.focus != pane else { return false }
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
