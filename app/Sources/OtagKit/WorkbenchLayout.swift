import Foundation

/// The window's layout, written down so that it survives a restart.
///
/// The daemon remembers the sessions; it does not know how the app arranged
/// them on screen, and it should not — that is the one piece of state the app
/// owns. Without this, quitting with three sessions side by side and
/// reopening showed one.
///
/// Every pane in a saved layout is checked against the sessions the daemon
/// is actually running before anything is drawn: a layout naming a session
/// that is over is a layout of what used to be.
public struct SavedLayout: Codable, Sendable, Equatable {
    /// How the showing sessions divided the space.
    public var root: SavedNode?
    /// Which of them had the keyboard.
    public var focus: PaneID?

    public init(root: SavedNode?, focus: PaneID?) {
        self.root = root
        self.focus = focus
    }
}

/// How a saved layout's panes divided the space.
///
/// A flat shape rather than the enum itself: `PaneNode` is an indirect enum,
/// and writing its JSON by hand is more code than a struct that says the
/// same thing. One of `pane` or `children` is set.
public struct SavedNode: Codable, Sendable, Equatable {
    /// The session filling this space, for a leaf.
    public var pane: PaneID?
    /// Whether a split divides its space side by side.
    public var horizontal: Bool?
    /// What a split is divided into.
    public var children: [SavedNode]?
    /// How those children share the space.
    public var fractions: [Double]?

    public init(
        pane: PaneID? = nil,
        horizontal: Bool? = nil,
        children: [SavedNode]? = nil,
        fractions: [Double]? = nil
    ) {
        self.pane = pane
        self.horizontal = horizontal
        self.children = children
        self.fractions = fractions
    }
}

public extension PaneNode {
    /// This node, as something that can be written down.
    var saved: SavedNode {
        switch self {
        case let .leaf(pane):
            return SavedNode(pane: pane)
        case let .split(axis, children, fractions):
            return SavedNode(
                horizontal: axis == .horizontal,
                children: children.map(\.saved),
                fractions: fractions
            )
        }
    }

    /// A node read back from a saved layout, keeping only the sessions in
    /// `alive`.
    ///
    /// `nil` when nothing in it is still running, which is how the window
    /// learns it has nothing left to show. A split that loses all but one
    /// child becomes that child, exactly as closing a pane does.
    static func restored(from saved: SavedNode, alive: Set<PaneID>) -> PaneNode? {
        if let pane = saved.pane {
            return alive.contains(pane) ? .leaf(pane) : nil
        }
        guard let children = saved.children, !children.isEmpty else { return nil }

        let fractions = saved.fractions ?? []
        var keptChildren: [PaneNode] = []
        var keptFractions: [Double] = []
        for (index, child) in children.enumerated() {
            guard let kept = restored(from: child, alive: alive) else { continue }
            keptChildren.append(kept)
            keptFractions.append(fractions.indices.contains(index) ? fractions[index] : 1)
        }

        switch keptChildren.count {
        case 0:
            return nil
        case 1:
            return keptChildren[0]
        default:
            let total = keptFractions.reduce(0, +)
            let shares = total > 0
                ? keptFractions.map { $0 / total }
                : Array(repeating: 1 / Double(keptChildren.count), count: keptChildren.count)
            return .split(
                axis: (saved.horizontal ?? true) ? .horizontal : .vertical,
                children: keptChildren,
                fractions: shares
            )
        }
    }
}

@MainActor
public extension Workbench {
    /// Where the layout is kept between runs.
    ///
    /// A different key from the one the tabbed window used: a layout saved
    /// by that build describes tabs this one has no idea what to do with, so
    /// it is ignored rather than half-read.
    static let layoutKey = "windowLayout2"

    /// The layout as it stands, for writing down.
    var saved: SavedLayout {
        SavedLayout(root: root?.saved, focus: focus)
    }

    /// Write the layout to `defaults`.
    func save(to defaults: UserDefaults = .standard) {
        guard let data = try? JSONEncoder().encode(saved) else { return }
        defaults.set(data, forKey: Self.layoutKey)
    }

    /// Put back the layout that was saved, keeping only the sessions in
    /// `alive`.
    ///
    /// Does nothing once anything is showing: this is for the moment the app
    /// first hears from the daemon, and a layout must never replace what the
    /// user is already looking at.
    func restore(from defaults: UserDefaults = .standard, alive: [PaneID]) {
        guard root == nil, let data = defaults.data(forKey: Self.layoutKey) else { return }
        // A layout this build cannot read is not an error; the window simply
        // starts empty, which is where it started before any of this.
        guard let layout = try? JSONDecoder().decode(SavedLayout.self, from: data) else { return }
        restore(layout, alive: alive)
    }

    /// Put back one layout, keeping only the sessions in `alive`.
    func restore(_ layout: SavedLayout, alive: [PaneID]) {
        guard root == nil, let saved = layout.root else { return }
        guard let tree = PaneNode.restored(from: saved, alive: Set(alive)) else { return }
        adopt(root: tree, focus: layout.focus ?? tree.panes[0])
    }
}
