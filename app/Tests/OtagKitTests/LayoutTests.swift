import Foundation
import Testing

@testable import OtagKit

/// Putting sessions side by side, and keeping them there.
@Suite("Sessions side by side")
@MainActor
struct SideBySideTests {
    @Test("a session dropped on an edge divides the whole area")
    func droppingOnEachEdge() throws {
        for (edge, expected) in [
            (DropEdge.trailing, [1 as PaneID, 2]),
            (DropEdge.leading, [2, 1]),
            (DropEdge.bottom, [1, 2]),
        ] {
            let workbench = Workbench()
            workbench.select(pane: 1)
            workbench.place(pane: 2, on: edge)

            #expect(workbench.panes == expected, "\(edge.label) put them in the wrong order")
            #expect(workbench.focus == 2, "the session just placed takes the keyboard")
            guard case let .split(axis, _, _) = try #require(workbench.root) else {
                Issue.record("\(edge.label) did not split")
                return
            }
            #expect(axis == edge.axis)
        }
    }

    @Test("there is no edge above: that is where the header is")
    func noTopEdge() {
        #expect(DropEdge.allCases.count == 3)
        #expect(Set(DropEdge.allCases) == [.leading, .trailing, .bottom])
    }

    @Test("three sessions side by side are three even columns")
    func threeAcross() throws {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.place(pane: 2, on: .trailing)
        workbench.place(pane: 3, on: .trailing)

        #expect(workbench.panes == [1, 2, 3])
        guard case let .split(_, children, fractions) = try #require(workbench.root) else {
            Issue.record("expected one row")
            return
        }
        #expect(children.count == 3)
        for share in fractions {
            #expect(abs(share - 1.0 / 3) < 0.0001, "the three columns are not even")
        }
    }

    @Test("a session dropped on the other side is moved, not shown twice")
    func draggingWithinTheWindow() {
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.place(pane: 2, on: .trailing)

        // The same session, dragged to the other side.
        workbench.place(pane: 2, on: .leading)

        #expect(workbench.panes == [2, 1], "it moved")
        #expect(workbench.panes.count == 2, "and is not in there twice")
    }

    @Test("a session dropped with nothing showing becomes the first thing")
    func droppingIntoAnEmptyWindow() {
        let workbench = Workbench()
        workbench.place(pane: 9, on: .trailing)

        #expect(workbench.panes == [9])
        #expect(workbench.focus == 9)
    }

    @Test("the arrangement comes back after a restart, dividers and all")
    func savingAndRestoring() throws {
        let suite = "otag.layout.test.\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        let before = Workbench()
        before.select(pane: 1)
        before.place(pane: 2, on: .trailing)
        before.move(divider: 0, of: try #require(before.root).id, by: 0.2)
        before.focus(pane: 1)
        before.save(to: defaults)

        let after = Workbench()
        after.restore(from: defaults, alive: [1, 2])

        #expect(after.panes == [1, 2])
        #expect(after.focus == 1, "the session that had the keyboard still has it")
        #expect(after.root == before.root, "including where the divider was dragged to")
    }

    @Test("a saved session the daemon is no longer running is left out")
    func restoringWithoutADeadSession() {
        let workbench = Workbench()
        workbench.restore(
            SavedLayout(
                root: SavedNode(
                    horizontal: true,
                    children: [SavedNode(pane: 1), SavedNode(pane: 2)],
                    fractions: [0.5, 0.5]
                ),
                focus: 2
            ),
            alive: [1]
        )

        #expect(workbench.panes == [1], "only the one still running")
        #expect(workbench.root == .leaf(1), "and it is not a split of one")
        #expect(workbench.focus == 1, "the keyboard went to a session that is here")
    }

    @Test("a layout of sessions that are all gone leaves the window empty")
    func restoringNothing() {
        let workbench = Workbench()
        workbench.restore(
            SavedLayout(root: SavedNode(pane: 1), focus: 1),
            alive: [7]
        )
        #expect(workbench.root == nil)
        #expect(workbench.focus == nil)
    }

    @Test("a layout never replaces what the user is already looking at")
    func restoringOverSomethingOpen() {
        let workbench = Workbench()
        workbench.select(pane: 5)
        workbench.restore(SavedLayout(root: SavedNode(pane: 1), focus: 1), alive: [1, 5])

        #expect(workbench.panes == [5])
    }

    @Test("the layout the tabbed window saved is ignored rather than half-read")
    func anOldLayoutIsNotRead() throws {
        let suite = "otag.layout.old.\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        // What the previous build wrote: a list of tabs, under its own key.
        let old = #"{"tabs":[{"id":"x","title":"claude","root":{"pane":1},"focus":1}]}"#
        defaults.set(Data(old.utf8), forKey: "windowLayout")

        let workbench = Workbench()
        workbench.restore(from: defaults, alive: [1])
        #expect(workbench.root == nil, "nothing was read from the old key")
    }
}
