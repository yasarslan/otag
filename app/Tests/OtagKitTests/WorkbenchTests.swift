import Foundation
import Testing

@testable import OtagKit

/// The layout is the one piece of state the app owns rather than reads from
/// the daemon, so it is the one worth testing on its own.
///
/// There are no tabs: the sidebar selects, and the window shows one session
/// or up to three side by side.
@Suite("The showing sessions")
@MainActor
struct WorkbenchTests {
    @Test("selecting a session shows it on its own")
    func selecting() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        #expect(workbench.panes == [1])
        #expect(workbench.focus == 1)

        workbench.select(pane: 2)
        #expect(workbench.panes == [2], "the second replaces the first")
        #expect(workbench.focus == 2)
    }

    @Test("selecting any session always ends up showing that session")
    func selectingAlwaysShowsIt() {
        // The bug this pins: picking a session in the sidebar left the main
        // area showing a different one. Whatever the window was showing
        // first, the session selected has to be both showing and focused
        // afterwards — there is no state in which selecting does nothing.
        let starts: [(String, (Workbench) -> Void)] = [
            ("nothing showing", { _ in }),
            ("another session showing", { $0.select(pane: 9) }),
            ("a split it is not in", { bench in
                bench.select(pane: 9)
                bench.place(pane: 8, on: .trailing)
            }),
            ("a split it is in", { bench in
                bench.select(pane: 9)
                bench.place(pane: 7, on: .trailing)
            }),
            ("three sessions", { bench in
                bench.select(pane: 9)
                bench.place(pane: 8, on: .trailing)
                bench.place(pane: 7, on: .bottom)
            }),
        ]

        for (what, arrange) in starts {
            let workbench = Workbench()
            arrange(workbench)
            workbench.select(pane: 7)

            #expect(workbench.isOpen(7), "with \(what), session 7 is not showing")
            #expect(workbench.focus == 7, "with \(what), session 7 does not have the keyboard")
        }
    }

    @Test("a leaf's identity follows the session in it")
    func identityFollowsTheSession() {
        // What the view keys its terminal on. A terminal holds one session's
        // byte stream, so a view reused across a change of session goes on
        // showing the old one: the identity has to change with the session
        // for the old view to be taken down and a new one put up.
        #expect(PaneNode.leaf(1).id != PaneNode.leaf(2).id)
        #expect(PaneNode.leaf(1).id == PaneNode.leaf(1).id)

        let before = PaneNode.split(axis: .horizontal, children: [.leaf(1), .leaf(2)],
                                    fractions: [0.5, 0.5])
        let after = PaneNode.split(axis: .horizontal, children: [.leaf(1), .leaf(3)],
                                   fractions: [0.5, 0.5])
        #expect(before.id != after.id, "a split that swapped a session is not the same split")
    }

    @Test("selecting a session already side by side only moves the keyboard")
    func selectingOneAlreadyShowing() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.place(pane: 2, on: .trailing)

        workbench.select(pane: 1)
        #expect(workbench.panes == [1, 2], "the arrangement is left alone")
        #expect(workbench.focus == 1, "and the keyboard moves")
    }

    @Test("splitting divides the focused pane's space")
    func splitting() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.split(with: 2, axis: .horizontal)

        guard case let .split(axis, children, fractions) = workbench.root else {
            Issue.record("expected a split, got \(String(describing: workbench.root))")
            return
        }
        #expect(axis == .horizontal)
        #expect(children == [.leaf(1), .leaf(2)])
        #expect(fractions == [0.5, 0.5])
        #expect(workbench.focus == 2, "the new pane takes the keyboard")
    }

    @Test("splitting with nothing showing simply shows it")
    func splittingFromEmpty() {
        let workbench = Workbench()
        workbench.split(with: 7, axis: .horizontal)
        #expect(workbench.panes == [7])
        #expect(workbench.focus == 7)
    }

    @Test("a session cannot be split with itself")
    func splittingWithItself() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.split(with: 1, axis: .horizontal)
        #expect(workbench.panes == [1])
    }

    @Test("four sessions fit, and a fifth replaces the one left longest")
    func fourAtMost() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.place(pane: 2, on: .trailing)
        workbench.place(pane: 3, on: .trailing)
        workbench.place(pane: 4, on: .bottom)
        #expect(workbench.panes.count == 4)
        #expect(workbench.refusal == nil, "four is allowed and says nothing")

        workbench.place(pane: 5, on: .trailing)
        #expect(workbench.panes.count == Workbench.maximumPanes)
        #expect(!workbench.panes.contains(1), "the one nobody had looked at went")
        #expect(workbench.panes.contains(5))
        #expect(workbench.refusal != nil, "and the window is told why")
    }

    @Test("looking at a pane keeps it from being the one replaced")
    func focusingSparesAPane() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.place(pane: 2, on: .trailing)
        workbench.place(pane: 3, on: .trailing)
        workbench.place(pane: 4, on: .bottom)

        // Back to the first one, which makes the second the stalest.
        workbench.focus(pane: 1)
        workbench.place(pane: 5, on: .trailing)

        #expect(workbench.panes.contains(1), "the one just looked at stayed")
        #expect(!workbench.panes.contains(2))
    }

    @Test("closing a pane gives its space to its neighbours")
    func closing() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.split(with: 2, axis: .horizontal)
        workbench.close(pane: 2)

        #expect(workbench.root == .leaf(1), "a split with one child left is not a split")
        #expect(workbench.focus == 1, "the keyboard went somewhere real")
    }

    @Test("closing the last pane leaves the window empty")
    func closingTheLast() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.close(pane: 1)

        #expect(workbench.root == nil)
        #expect(workbench.focus == nil)
        #expect(workbench.panes.isEmpty)
    }

    @Test("the keyboard steps through the panes in the order they are drawn")
    func steppingFocus() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.place(pane: 2, on: .trailing)
        workbench.place(pane: 3, on: .trailing)
        workbench.focus(pane: 1)

        workbench.stepFocus(by: 1)
        #expect(workbench.focus == 2)
        workbench.stepFocus(by: -1)
        #expect(workbench.focus == 1)
        workbench.stepFocus(by: -1)
        #expect(workbench.focus == 3, "and wraps around")
    }

    @Test("one pane has nothing to step to")
    func steppingWithOnePane() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.stepFocus(by: 1)
        #expect(workbench.focus == 1)
    }

    @Test("a divider moves without moving the others")
    func movingADivider() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.place(pane: 2, on: .trailing)
        workbench.place(pane: 3, on: .trailing)

        guard let id = workbench.root?.id else {
            Issue.record("no root")
            return
        }
        workbench.move(divider: 0, of: id, by: 0.1)

        guard case let .split(_, children, fractions) = workbench.root else {
            Issue.record("expected a split")
            return
        }
        #expect(children.count == 3, "three across is one row, not a nest")
        #expect(abs(fractions.reduce(0, +) - 1) < 0.0001, "the shares still add up")
        #expect(fractions[0] > fractions[1], "the first divider moved")
        #expect(abs(fractions[2] - 1.0 / 3) < 0.0001, "and the third pane did not")
    }
}
