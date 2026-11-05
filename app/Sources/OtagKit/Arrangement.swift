import Foundation

/// Laying several sessions out evenly, without anybody dragging anything.
///
/// Dragging a session onto an edge says exactly where it goes, which is the
/// right tool when you know. Arrange is for the other case: four sessions in
/// one project and no wish to place each of them by hand.
///
/// A pure function, so every shape can be checked without a window. The
/// shapes are the ones that use a rectangular screen well, which is why
/// three is not three columns: three columns of a terminal are too narrow to
/// read, and one half beside two quarters is not.
public enum Arrangement {
    /// How many sessions fit on one screen and stay readable.
    ///
    /// Four, which is the 2×2. More than that and a terminal is too small to
    /// work in, so the four most recently used are the ones shown.
    public static let most = 4

    /// The tree that lays these sessions out evenly.
    ///
    /// `panes` is most recently used first; anything past the fourth is left
    /// out. `nil` when there is nothing to lay out.
    ///
    /// ```text
    /// 1: ┌───────┐  2: ┌───┬───┐  3: ┌───┬───┐  4: ┌───┬───┐
    ///    │   A   │     │ A │ B │     │   │ B │     │ A │ B │
    ///    │       │     │   │   │     │ A ├───┤     ├───┼───┤
    ///    │       │     │   │   │     │   │ C │     │ C │ D │
    ///    └───────┘     └───┴───┘     └───┴───┘     └───┴───┘
    /// ```
    public static func of(_ panes: [PaneID]) -> PaneNode? {
        // The order the panes are drawn in is the order they were given, so
        // the most recently used session is top left — where the eye starts.
        let kept = Array(panes.prefix(most))

        switch kept.count {
        case 0:
            return nil

        case 1:
            return .leaf(kept[0])

        case 2:
            return row(kept[0], kept[1])

        case 3:
            // One half, and two quarters stacked beside it. Three columns
            // would each be a third of the width, which no terminal reads
            // well.
            return .split(
                axis: .horizontal,
                children: [.leaf(kept[0]), column(kept[1], kept[2])],
                fractions: [0.5, 0.5]
            )

        default:
            // Two rows of two: a grid, and the only even way to divide four.
            return .split(
                axis: .vertical,
                children: [row(kept[0], kept[1]), row(kept[2], kept[3])],
                fractions: [0.5, 0.5]
            )
        }
    }

    /// Two sessions side by side, each half the width.
    private static func row(_ left: PaneID, _ right: PaneID) -> PaneNode {
        .split(axis: .horizontal, children: [.leaf(left), .leaf(right)], fractions: [0.5, 0.5])
    }

    /// Two sessions stacked, each half the height.
    private static func column(_ top: PaneID, _ bottom: PaneID) -> PaneNode {
        .split(axis: .vertical, children: [.leaf(top), .leaf(bottom)], fractions: [0.5, 0.5])
    }
}
