import Foundation
import Testing

@testable import OtagKit

/// Laying sessions out evenly.
///
/// The shapes are the point, so the shapes are what these check: a session
/// in the wrong half is the kind of thing that looks fine in a screenshot of
/// one case and wrong in every other.
@Suite("Arranging sessions")
struct ArrangementTests {
    @Test("one session fills the window")
    func one() {
        #expect(Arrangement.of([1]) == .leaf(1))
    }

    @Test("nothing to arrange is nothing")
    func none() {
        #expect(Arrangement.of([]) == nil)
    }

    @Test("two sessions are halves, side by side")
    func two() throws {
        guard case let .split(axis, children, fractions) = try #require(Arrangement.of([1, 2]))
        else {
            Issue.record("expected a split")
            return
        }
        #expect(axis == .horizontal)
        #expect(children == [.leaf(1), .leaf(2)])
        #expect(fractions == [0.5, 0.5])
    }

    @Test("three sessions are one half beside two stacked quarters")
    func three() throws {
        // Not three columns: a third of a window is too narrow for a
        // terminal, and a half beside two quarters is not.
        guard case let .split(axis, children, fractions) = try #require(Arrangement.of([1, 2, 3]))
        else {
            Issue.record("expected a split")
            return
        }
        #expect(axis == .horizontal, "the first division is left and right")
        #expect(fractions == [0.5, 0.5])
        #expect(children.count == 2)
        #expect(children[0] == .leaf(1), "the newest is the big one, on the left")

        guard case let .split(stacked, pair, shares) = children[1] else {
            Issue.record("the right half is not divided")
            return
        }
        #expect(stacked == .vertical, "the right half is stacked, not side by side")
        #expect(pair == [.leaf(2), .leaf(3)])
        #expect(shares == [0.5, 0.5])
    }

    @Test("four sessions are a two by two grid")
    func four() throws {
        guard case let .split(axis, rows, fractions) = try #require(Arrangement.of([1, 2, 3, 4]))
        else {
            Issue.record("expected a split")
            return
        }
        #expect(axis == .vertical, "two rows")
        #expect(fractions == [0.5, 0.5])
        #expect(rows.count == 2)

        for (row, expected) in zip(rows, [[PaneNode.leaf(1), .leaf(2)], [.leaf(3), .leaf(4)]]) {
            guard case let .split(across, pair, shares) = row else {
                Issue.record("a row is not divided")
                return
            }
            #expect(across == .horizontal)
            #expect(pair == expected)
            #expect(shares == [0.5, 0.5])
        }
    }

    @Test("more than four shows the four most recently used")
    func moreThanFour() throws {
        let tree = try #require(Arrangement.of([9, 8, 7, 6, 5, 4, 3]))
        #expect(tree.panes == [9, 8, 7, 6], "the oldest three are left out")
        #expect(tree.panes.count == Arrangement.most)
    }

    @Test("every session given is drawn exactly once")
    func noneLostOrDoubled() throws {
        for count in 1 ... 6 {
            let wanted = Array(1 ... PaneID(count))
            let tree = try #require(Arrangement.of(wanted))
            let drawn = tree.panes
            #expect(
                Set(drawn).count == drawn.count,
                "\(count) sessions put one on screen twice"
            )
            #expect(drawn.count == min(count, Arrangement.most))
            for pane in drawn {
                #expect(wanted.contains(pane), "\(pane) came from nowhere")
            }
        }
    }

    @Test("the space always adds up to the whole window")
    func theSpaceAddsUp() throws {
        for count in 1 ... 5 {
            let tree = try #require(Arrangement.of(Array(1 ... PaneID(count))))
            check(tree, count: count)
        }
    }

    /// Every split in the tree divides its own space completely.
    private func check(_ node: PaneNode, count: Int) {
        guard case let .split(_, children, fractions) = node else { return }
        #expect(
            abs(fractions.reduce(0, +) - 1) < 0.0001,
            "with \(count) sessions a split does not fill its space"
        )
        #expect(children.count == fractions.count)
        for child in children {
            check(child, count: count)
        }
    }
}

/// Arrange, from the window's point of view.
@Suite("Arrange and back again")
@MainActor
struct ArrangeTests {
    @Test("arranging lays the sessions out and pressing again goes back")
    func thereAndBack() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        #expect(!workbench.isArranged)

        workbench.arrange([1, 2, 3], keeping: 1)
        #expect(workbench.panes == [1, 2, 3])
        #expect(workbench.isArranged)

        workbench.arrange([1, 2, 3], keeping: 1)
        #expect(workbench.panes == [1], "back to the one session")
        #expect(workbench.focus == 1)
        #expect(!workbench.isArranged)
    }

    @Test("going back lands on the session that was showing before")
    func backToWhatWasShowing() {
        let workbench = Workbench()
        workbench.select(pane: 7)
        workbench.arrange([7, 1, 2, 3], keeping: 7)
        // Look at another one while arranged.
        workbench.focus(pane: 2)

        workbench.arrange([7, 1, 2, 3], keeping: 2)
        #expect(workbench.panes == [7], "the one that was showing before Arrange")
    }

    @Test("going back when that session is gone lands somewhere real")
    func backWhenItIsGone() {
        let workbench = Workbench()
        workbench.select(pane: 7)
        workbench.arrange([7, 1, 2], keeping: 7)
        workbench.close(pane: 7)

        workbench.arrange([1, 2], keeping: 1)
        #expect(workbench.panes == [1])
        #expect(workbench.focus == 1)
    }

    @Test("dragging while arranged leaves the arrangement behind")
    func draggingEndsIt() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.arrange([1, 2], keeping: 1)
        #expect(workbench.isArranged)

        workbench.place(pane: 3, on: .trailing)
        #expect(!workbench.isArranged, "the layout is no longer the one Arrange made")
    }

    @Test("selecting a session while arranged leaves the arrangement behind")
    func selectingEndsIt() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.arrange([1, 2], keeping: 1)

        workbench.select(pane: 9)
        #expect(!workbench.isArranged)
        #expect(workbench.panes == [9])
    }

    @Test("the recently used order puts the newest first")
    func recentlyUsed() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.place(pane: 2, on: .trailing)
        workbench.place(pane: 3, on: .trailing)
        workbench.focus(pane: 1)

        #expect(
            workbench.recentlyUsed == [1, 3, 2],
            "the one just looked at first, then back through the others"
        )
    }

    @Test("arranging nothing changes nothing")
    func arrangingNothing() {
        let workbench = Workbench()
        workbench.arrange([], keeping: nil)
        #expect(workbench.root == nil)
        #expect(!workbench.isArranged)
    }

    @Test("four sessions fit, and a fifth replaces the one left longest")
    func fourFit() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.place(pane: 2, on: .trailing)
        workbench.place(pane: 3, on: .trailing)
        workbench.place(pane: 4, on: .bottom)
        #expect(workbench.panes.count == 4, "a 2x2 needs four")
        #expect(workbench.refusal == nil)

        workbench.place(pane: 5, on: .trailing)
        #expect(workbench.panes.count == Workbench.maximumPanes)
        #expect(!workbench.panes.contains(1))
        #expect(workbench.refusal != nil)
    }
}
