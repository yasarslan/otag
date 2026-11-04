import Foundation
import Observation

/// Which way a split divides the space it is given.
public enum SplitAxis: Sendable, Hashable {
    /// Side by side.
    case horizontal
    /// One above the other.
    case vertical
}

/// How the panes of one tab divide its space.
///
/// A tree, because that is what splitting a pane produces: splitting one half
/// again divides that half, not the whole tab. Children of a split share its
/// space in the fractions it carries, so dragging one divider never moves the
/// others.
public enum PaneNode: Sendable, Hashable, Identifiable {
    /// One pane, filling its space.
    case leaf(PaneID)
    /// Several nodes, side by side or stacked.
    indirect case split(axis: SplitAxis, children: [PaneNode], fractions: [Double])

    public var id: String {
        switch self {
        case let .leaf(pane):
            return "pane-\(pane)"
        case let .split(axis, children, _):
            let kind = axis == .horizontal ? "h" : "v"
            return "\(kind)(\(children.map(\.id).joined(separator: ",")))"
        }
    }

    /// Every pane in this node, left to right and top to bottom.
    public var panes: [PaneID] {
        switch self {
        case let .leaf(pane):
            return [pane]
        case let .split(_, children, _):
            return children.flatMap(\.panes)
        }
    }

    /// Put `pane` beside `beside`, dividing only the space `beside` has.
    ///
    /// Returns whether `beside` was found: a split asked for a pane that is
    /// not in this tab changes nothing.
    public mutating func split(beside: PaneID, with pane: PaneID, axis: SplitAxis) -> Bool {
        switch self {
        case let .leaf(existing):
            guard existing == beside else { return false }
            self = .split(axis: axis, children: [.leaf(existing), .leaf(pane)],
                          fractions: [0.5, 0.5])
            return true

        case .split(let ownAxis, var children, var fractions):
            for index in children.indices {
                // A leaf in a split along the same axis joins that split
                // rather than nesting one inside it, which is what keeps a
                // row of three panes a row of three.
                if case let .leaf(existing) = children[index], existing == beside,
                   ownAxis == axis {
                    let share = fractions[index] / 2
                    fractions[index] = share
                    fractions.insert(share, at: index + 1)
                    children.insert(.leaf(pane), at: index + 1)
                    self = .split(axis: ownAxis, children: children, fractions: fractions)
                    return true
                }
                var child = children[index]
                if child.split(beside: beside, with: pane, axis: axis) {
                    children[index] = child
                    self = .split(axis: ownAxis, children: children, fractions: fractions)
                    return true
                }
            }
            return false
        }
    }

    /// Take `pane` out, giving its space to its neighbours.
    ///
    /// `nil` once nothing is left, which is how a tab learns it should close.
    public func removing(_ pane: PaneID) -> PaneNode? {
        switch self {
        case let .leaf(existing):
            return existing == pane ? nil : self

        case let .split(axis, children, fractions):
            var keptChildren: [PaneNode] = []
            var keptFractions: [Double] = []
            for (index, child) in children.enumerated() {
                guard let kept = child.removing(pane) else { continue }
                keptChildren.append(kept)
                keptFractions.append(fractions.indices.contains(index) ? fractions[index] : 0)
            }

            switch keptChildren.count {
            case 0:
                return nil
            // A split with one child left is not a split any more.
            case 1:
                return keptChildren[0]
            default:
                return .split(axis: axis, children: keptChildren,
                              fractions: normalised(keptFractions))
            }
        }
    }

    /// Move the divider after `index` of the split identified by `id`.
    ///
    /// `delta` is a fraction of that split's own space, so dragging feels the
    /// same whether the split is the whole tab or a corner of it.
    public mutating func move(divider index: Int, of id: String, by delta: Double) {
        switch self {
        case .leaf:
            return

        case .split(let axis, var children, var fractions):
            if self.id == id {
                guard fractions.indices.contains(index),
                      fractions.indices.contains(index + 1) else { return }
                /// Neither side may be dragged away entirely.
                let smallest = 0.05
                let total = fractions[index] + fractions[index + 1]
                let wanted = min(max(fractions[index] + delta, smallest), total - smallest)
                fractions[index] = wanted
                fractions[index + 1] = total - wanted
                self = .split(axis: axis, children: children, fractions: fractions)
                return
            }
            for position in children.indices {
                var child = children[position]
                child.move(divider: index, of: id, by: delta)
                children[position] = child
            }
            self = .split(axis: axis, children: children, fractions: fractions)
        }
    }

    /// This node and `pane` side by side, with `pane` on `edge`.
    ///
    /// What a session dropped on the edge of the terminal area means: it
    /// divides the whole tab rather than one pane of it, which is why this
    /// works on the root and the dragging never has to say which pane it
    /// landed over.
    public func splitting(off edge: DropEdge, with pane: PaneID) -> PaneNode {
        let leaf = PaneNode.leaf(pane)
        let children = edge.putsItFirst ? [leaf, self] : [self, leaf]
        return .split(axis: edge.axis, children: children, fractions: [0.5, 0.5])
    }

    /// Fractions that add up to one, however they were cut up.
    private static func normalise(_ fractions: [Double]) -> [Double] {
        let total = fractions.reduce(0, +)
        guard total > 0 else {
            return Array(repeating: 1 / Double(fractions.count), count: fractions.count)
        }
        return fractions.map { $0 / total }
    }

    private func normalised(_ fractions: [Double]) -> [Double] {
        Self.normalise(fractions)
    }
}

/// Which edge of the terminal area a session was dropped on.
///
/// Four edges rather than two axes, because "left" and "right" are what the
/// user is aiming at; the axis and the order follow from it.
public enum DropEdge: Sendable, Hashable, CaseIterable {
    case leading
    case trailing
    case top
    case bottom

    /// Which way the split divides the space.
    public var axis: SplitAxis {
        switch self {
        case .leading, .trailing: return .horizontal
        case .top, .bottom: return .vertical
        }
    }

    /// Whether the dropped session goes before what is already there.
    public var putsItFirst: Bool {
        self == .leading || self == .top
    }
}

/// One tab: a set of panes and which of them has the keyboard.
public struct TerminalTab: Sendable, Identifiable, Hashable {
    public let id: UUID
    /// What the tab strip calls it.
    public var title: String
    /// How its panes divide the space.
    public var root: PaneNode
    /// The pane the keyboard goes to.
    public var focus: PaneID

    public init(id: UUID = UUID(), title: String, pane: PaneID) {
        self.id = id
        self.title = title
        root = .leaf(pane)
        focus = pane
    }

    /// A tab with a whole arrangement, for one read back from a saved
    /// layout.
    public init(id: UUID, title: String, root: PaneNode, focus: PaneID) {
        self.id = id
        self.title = title
        self.root = root
        self.focus = focus
    }

    /// Every pane in the tab.
    public var panes: [PaneID] { root.panes }
}

/// The tabs a window is showing, and which pane has the keyboard.
///
/// This is the app's own layout, not the daemon's: the daemon owns the panes
/// and their sizes, and the app decides where on screen they go. That is the
/// one piece of state the app keeps for itself, so it is also the one worth
/// testing on its own.
@MainActor
@Observable
public final class Workbench {
    /// The open tabs, left to right.
    public private(set) var tabs: [TerminalTab] = []
    /// Which tab is shown.
    public var selected: UUID?

    public init() {}

    /// The tab that is shown.
    public var current: TerminalTab? {
        tabs.first { $0.id == selected }
    }

    /// The pane the keyboard goes to.
    public var focus: PaneID? {
        current?.focus
    }

    /// Whether any tab is showing this pane.
    public func isOpen(_ pane: PaneID) -> Bool {
        tabs.contains { $0.panes.contains(pane) }
    }

    /// Show `pane`: its tab if it already has one, a new tab otherwise.
    public func open(pane: PaneID, title: String) {
        if let tab = tabs.first(where: { $0.panes.contains(pane) }) {
            selected = tab.id
            focus(pane: pane)
            return
        }
        let tab = TerminalTab(title: title, pane: pane)
        tabs.append(tab)
        selected = tab.id
    }

    /// Give the keyboard to `pane`, in whichever tab holds it.
    public func focus(pane: PaneID) {
        guard let index = tabs.firstIndex(where: { $0.panes.contains(pane) }) else { return }
        tabs[index].focus = pane
        selected = tabs[index].id
    }

    /// Put `pane` beside the focused one in the current tab.
    public func split(with pane: PaneID, axis: SplitAxis) {
        guard let index = tabs.firstIndex(where: { $0.id == selected }) else { return }
        var tab = tabs[index]
        guard tab.root.split(beside: tab.focus, with: pane, axis: axis) else { return }
        tab.focus = pane
        tabs[index] = tab
    }

    /// Take a pane out of whatever tab holds it, closing a tab that is left
    /// with nothing.
    ///
    /// This is what a pane whose program ended looks like from here: the
    /// daemon has already let it go.
    public func close(pane: PaneID) {
        for index in tabs.indices.reversed() {
            guard tabs[index].panes.contains(pane) else { continue }
            guard let root = tabs[index].root.removing(pane) else {
                let closing = tabs[index].id
                tabs.remove(at: index)
                if selected == closing {
                    selected = tabs.first?.id
                }
                continue
            }
            var tab = tabs[index]
            tab.root = root
            if tab.focus == pane {
                tab.focus = root.panes.first ?? pane
            }
            tabs[index] = tab
        }
    }

    /// Close a whole tab, and say which panes it held so the caller can let
    /// the daemon know nobody is watching them any more.
    @discardableResult
    public func closeTab(_ id: UUID) -> [PaneID] {
        guard let index = tabs.firstIndex(where: { $0.id == id }) else { return [] }
        let panes = tabs[index].panes
        tabs.remove(at: index)
        if selected == id {
            selected = tabs.indices.contains(index) ? tabs[index].id : tabs.last?.id
        }
        return panes
    }

    /// Take in a tab that was built elsewhere, which is how a saved layout
    /// is put back. Nothing else should need it.
    public func adopt(_ tab: TerminalTab) {
        guard !tab.panes.isEmpty, !tabs.contains(where: { $0.id == tab.id }) else { return }
        tabs.append(tab)
        if selected == nil {
            selected = tab.id
        }
    }

    /// Rename a tab, which is what a pane's title changing means.
    public func rename(tab id: UUID, to title: String) {
        guard let index = tabs.firstIndex(where: { $0.id == id }), tabs[index].title != title
        else { return }
        tabs[index].title = title
    }

    /// Move a divider of the current tab.
    public func move(divider index: Int, of node: String, by delta: Double) {
        guard let position = tabs.firstIndex(where: { $0.id == selected }) else { return }
        var tab = tabs[position]
        tab.root.move(divider: index, of: node, by: delta)
        tabs[position] = tab
    }

    /// Put an already running session into the current tab, on `edge`.
    ///
    /// What dragging a session from the sidebar onto the edge of the
    /// terminal area means. The edge divides the whole tab, so the drag does
    /// not have to say which pane it landed over — and a session already in
    /// this tab is *moved* rather than shown twice.
    public func place(pane: PaneID, on edge: DropEdge, title: String) {
        let target = selected

        // Out of whatever tab holds it first, so that dragging within one
        // tab rearranges rather than duplicating. A tab left with nothing
        // goes, which can be the tab being dropped on — so the tab is found
        // again afterwards, by identifier, rather than by position.
        if isOpen(pane) {
            close(pane: pane)
        }
        guard let index = tabs.firstIndex(where: { $0.id == target }) else {
            // Nothing left to put it beside, so it becomes the first thing.
            open(pane: pane, title: title)
            return
        }

        var tab = tabs[index]
        tab.root = tab.root.splitting(off: edge, with: pane)
        tab.focus = pane
        tabs[index] = tab
        selected = tab.id
    }

    /// Give the keyboard to the pane `offset` places along the current tab,
    /// wrapping around.
    ///
    /// The panes are in the order they are drawn, so this walks left to
    /// right and top to bottom, which is what the keys look like they do.
    public func stepFocus(by offset: Int) {
        guard let index = tabs.firstIndex(where: { $0.id == selected }) else { return }
        let panes = tabs[index].panes
        guard panes.count > 1 else { return }
        let at = panes.firstIndex(of: tabs[index].focus) ?? 0
        let count = panes.count
        tabs[index].focus = panes[((at + offset) % count + count) % count]
    }

    /// Show the tab `offset` places from the current one, wrapping around.
    public func step(by offset: Int) {
        guard !tabs.isEmpty else { return }
        let current = tabs.firstIndex { $0.id == selected } ?? 0
        let count = tabs.count
        let next = ((current + offset) % count + count) % count
        selected = tabs[next].id
    }
}
