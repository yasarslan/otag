import Foundation
import Observation

/// Which way a split divides the space it is given.
public enum SplitAxis: Sendable, Hashable {
    /// Side by side.
    case horizontal
    /// One above the other.
    case vertical
}

/// How the showing sessions divide the main area.
///
/// A tree, because that is what splitting produces: splitting one half again
/// divides that half, not the whole area. Children of a split share its
/// space in the fractions it carries, so dragging one divider never moves
/// the others.
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
    /// not showing changes nothing.
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
    /// `nil` once nothing is left, which is how the window learns it has
    /// nothing to show.
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
    /// same whether the split is the whole area or a corner of it.
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
    /// What a session dropped on the edge of the main area means: it divides
    /// the whole area rather than one pane of it, which is why this works on
    /// the root and the dragging never has to say which pane it landed over.
    public func splitting(off edge: DropEdge, with pane: PaneID) -> PaneNode {
        let leaf = PaneNode.leaf(pane)

        // A row already divided this way takes the newcomer in rather than
        // nesting a split inside itself, which is what keeps three sessions
        // dropped on the same edge three even columns instead of 25/25/50.
        if case let .split(axis, children, fractions) = self, axis == edge.axis {
            var kept = children
            var shares = fractions
            kept.insert(leaf, at: edge.putsItFirst ? 0 : kept.count)
            shares.insert(0, at: edge.putsItFirst ? 0 : shares.count)

            let even = 1 / Double(kept.count)
            let total = shares.reduce(0, +)
            // The newcomer takes an even share; the rest keep their ratios
            // to each other in what is left.
            var out = shares.map { total > 0 ? $0 / total * (1 - even) : even }
            out[edge.putsItFirst ? 0 : out.count - 1] = even
            return .split(axis: axis, children: kept, fractions: out)
        }

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

/// Which edge of the main area a session was dropped on.
///
/// Three edges, not four: "above" is the one nobody wants, and the top of
/// the area is where the header bar already is.
public enum DropEdge: Sendable, Hashable, CaseIterable {
    case leading
    case trailing
    case bottom

    /// Which way the split divides the space.
    public var axis: SplitAxis {
        switch self {
        case .leading, .trailing: return .horizontal
        case .bottom: return .vertical
        }
    }

    /// Whether the dropped session goes before what is already there.
    public var putsItFirst: Bool {
        self == .leading
    }

    /// What to call it, for a tooltip or a test's failure message.
    public var label: String {
        switch self {
        case .leading: return "left"
        case .trailing: return "right"
        case .bottom: return "bottom"
        }
    }
}

/// The sessions the window is showing, and which of them has the keyboard.
///
/// This is the app's own state, not the daemon's: the daemon owns the panes
/// and their sizes, and the app decides where on screen they go. One layout
/// of up to three panes — there are no tabs, because the sidebar is the only
/// navigator and a second navigator is how the window became hard to read.
@MainActor
@Observable
public final class Workbench {
    /// How the showing sessions divide the space, or nothing when none is
    /// showing.
    public private(set) var root: PaneNode?
    /// The pane the keyboard goes to.
    public private(set) var focus: PaneID?

    /// How many sessions may share the window.
    ///
    /// Four, which is the 2×2 that Arrange lays out. Past that a terminal is
    /// too small to work in, and a limit the user can feel beats a layout
    /// that keeps dividing.
    public static let maximumPanes = Arrangement.most

    /// The panes in the order they were last given the keyboard, oldest
    /// first. The one at the front is the one a fifth session replaces.
    private var leastRecentlyFocused: [PaneID] = []

    /// What the window was showing before Arrange, so pressing it again can
    /// put that back.
    ///
    /// Non-nil exactly while the window is arranged, which is what the
    /// toolbar button reads to know which way it will go.
    public private(set) var beforeArranging: PaneID?

    /// The last thing that could not be done, for the window to say once.
    public var refusal: String?

    public init() {}

    /// The showing sessions, left to right and top to bottom.
    public var panes: [PaneID] {
        root?.panes ?? []
    }

    /// Whether this session is showing.
    public func isOpen(_ pane: PaneID) -> Bool {
        panes.contains(pane)
    }

    /// Whether the window is showing an arrangement rather than a session
    /// somebody chose.
    public var isArranged: Bool {
        beforeArranging != nil
    }

    /// The sessions the window is showing, most recently used first.
    ///
    /// What Arrange lays out, and what decides which session a sixth one
    /// replaces. One order, used for both, so "recently used" means one
    /// thing in otag.
    public var recentlyUsed: [PaneID] {
        let showing = Set(panes)
        var order = leastRecentlyFocused.filter(showing.contains).reversed().map { $0 }
        // A pane that has somehow never been focused still has to be in the
        // list, or Arrange would quietly drop it.
        order.append(contentsOf: panes.filter { !order.contains($0) })
        return order
    }

    /// Lay `wanted` out evenly, or put back what was showing before.
    ///
    /// `wanted` is in the order the caller wants them drawn — most recently
    /// used first. Pressing Arrange again is what `keeping` is for: the
    /// session to go back to.
    public func arrange(_ wanted: [PaneID], keeping: PaneID?) {
        if isArranged {
            // Back to one session: the one that was showing before, if it is
            // still running, and otherwise whatever has the keyboard.
            let back = beforeArranging.flatMap { pane in
                wanted.contains(pane) ? pane : nil
            } ?? keeping ?? focus
            beforeArranging = nil
            guard let back else { return }
            // Set directly rather than through `select`, which leaves a
            // session that is already on screen exactly where it is — true
            // when the user picks one in the sidebar, wrong here, where the
            // whole point is to stop showing the others.
            root = .leaf(back)
            leastRecentlyFocused = []
            focus(pane: back)
            return
        }

        guard let tree = Arrangement.of(wanted) else { return }
        let was = keeping ?? focus
        root = tree
        // Everything on screen counts as used, newest first, so the order
        // survives going back and forth.
        leastRecentlyFocused = tree.panes.reversed()
        beforeArranging = was
        focus(pane: was.flatMap { tree.panes.contains($0) ? $0 : nil } ?? tree.panes[0])
    }

    /// Show `pane` on its own, replacing whatever was showing.
    ///
    /// What selecting a session in the sidebar means. A session already
    /// showing beside others is not torn out of the arrangement — it is
    /// simply given the keyboard, because the user asked to look at it, not
    /// to rearrange the window.
    public func select(pane: PaneID) {
        if isOpen(pane) {
            focus(pane: pane)
            return
        }
        root = .leaf(pane)
        leastRecentlyFocused = []
        beforeArranging = nil
        focus(pane: pane)
    }

    /// Give the keyboard to `pane`.
    public func focus(pane: PaneID) {
        guard isOpen(pane) else { return }
        focus = pane
        leastRecentlyFocused.removeAll { $0 == pane }
        leastRecentlyFocused.append(pane)
    }

    /// Put `pane` beside the focused one, on this axis.
    ///
    /// What `Split with…`, `⌘D` and `⇧⌘D` mean.
    public func split(with pane: PaneID, axis: SplitAxis) {
        guard let beside = focus, let existing = root else {
            select(pane: pane)
            return
        }
        guard pane != beside else { return }
        guard makeRoom(for: pane) else { return }

        var node = existing
        guard node.split(beside: beside, with: pane, axis: axis) else { return }
        root = node
        beforeArranging = nil
        focus(pane: pane)
    }

    /// Put an already running session into the window, on `edge`.
    ///
    /// What dragging a session from the sidebar onto the side or the bottom
    /// of the main area means. The edge divides the whole area, so the drag
    /// does not have to say which pane it landed over — and a session
    /// already showing is *moved* rather than shown twice.
    public func place(pane: PaneID, on edge: DropEdge) {
        // Out first, so that dragging within the window rearranges rather
        // than duplicating.
        if isOpen(pane) {
            close(pane: pane)
        }
        guard root != nil, makeRoom(for: pane), let kept = root else {
            // Nothing left to put it beside, so it becomes the first thing.
            select(pane: pane)
            return
        }
        root = kept.splitting(off: edge, with: pane)
        beforeArranging = nil
        focus(pane: pane)
    }

    /// Stop showing `pane`, giving its space to its neighbours.
    ///
    /// This is also what a pane whose program ended looks like from here:
    /// the daemon has already let it go.
    public func close(pane: PaneID) {
        guard isOpen(pane) else { return }
        leastRecentlyFocused.removeAll { $0 == pane }
        root = root?.removing(pane)
        if focus == pane {
            focus = leastRecentlyFocused.last ?? root?.panes.first
        }
        if root == nil {
            focus = nil
            leastRecentlyFocused = []
            beforeArranging = nil
        }
    }

    /// Move a divider.
    public func move(divider index: Int, of node: String, by delta: Double) {
        guard var tree = root else { return }
        tree.move(divider: index, of: node, by: delta)
        root = tree
    }

    /// Give the keyboard to the pane `offset` places along, wrapping around.
    ///
    /// The panes are in the order they are drawn, so this walks left to
    /// right and top to bottom, which is what the keys look like they do.
    public func stepFocus(by offset: Int) {
        let showing = panes
        guard showing.count > 1, let current = focus,
              let at = showing.firstIndex(of: current) else { return }
        let count = showing.count
        focus(pane: showing[((at + offset) % count + count) % count])
    }

    /// Forget the last refusal, once the window has said it.
    public func clearRefusal() {
        refusal = nil
    }

    /// Make sure a fourth session has somewhere to go.
    ///
    /// The least recently used pane leaves rather than the drop failing
    /// silently — and the window says so, because a session disappearing
    /// without a word is worse than a refusal.
    private func makeRoom(for pane: PaneID) -> Bool {
        guard panes.count >= Self.maximumPanes else { return true }
        guard let evicted = leastRecentlyFocused.first ?? panes.first else { return false }
        guard evicted != pane else { return false }
        close(pane: evicted)
        refusal = Strings.t("window.tooMany", Self.maximumPanes)
        return true
    }

    /// Take in a layout built elsewhere, which is how a saved one is put
    /// back. Nothing else should need it.
    func adopt(root: PaneNode, focus: PaneID) {
        guard self.root == nil else { return }
        self.root = root
        self.focus = root.panes.contains(focus) ? focus : root.panes.first
        leastRecentlyFocused = root.panes
        if let wanted = self.focus {
            leastRecentlyFocused.removeAll { $0 == wanted }
            leastRecentlyFocused.append(wanted)
        }
    }
}
