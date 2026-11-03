import Foundation
import Testing

@testable import OtagKit

/// The layout is the one piece of state the app owns rather than reads from
/// the daemon, so it is the one worth testing on its own.
@Suite("Tabs and splits")
@MainActor
struct WorkbenchTests {
    @Test("a pane opens in a tab of its own, and opening it again just shows it")
    func openingAPane() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "claude")
        workbench.open(pane: 2, title: "codex")

        #expect(workbench.tabs.count == 2)
        #expect(workbench.selected == workbench.tabs[1].id)

        workbench.open(pane: 1, title: "claude")
        #expect(workbench.tabs.count == 2, "no second tab for the same pane")
        #expect(workbench.selected == workbench.tabs[0].id)
        #expect(workbench.focus == 1)
    }

    @Test("splitting divides the focused pane's space, not the whole tab")
    func splitting() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "claude")
        workbench.split(with: 2, axis: .horizontal)

        guard case let .split(axis, children, fractions) = workbench.current?.root else {
            Issue.record("expected a split, got \(String(describing: workbench.current?.root))")
            return
        }
        #expect(axis == .horizontal)
        #expect(children == [.leaf(1), .leaf(2)])
        #expect(fractions == [0.5, 0.5])
        #expect(workbench.focus == 2, "the new pane takes the keyboard")

        // A third pane split the same way joins the row rather than nesting.
        workbench.split(with: 3, axis: .horizontal)
        guard case let .split(_, row, shares) = workbench.current?.root else {
            Issue.record("expected a row")
            return
        }
        #expect(row == [.leaf(1), .leaf(2), .leaf(3)])
        #expect(shares == [0.5, 0.25, 0.25])
    }

    @Test("a split across the other axis nests inside the pane it divides")
    func nestedSplit() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "shell")
        workbench.split(with: 2, axis: .horizontal)
        workbench.split(with: 3, axis: .vertical)

        guard case let .split(.horizontal, children, _) = workbench.current?.root else {
            Issue.record("expected a horizontal split")
            return
        }
        #expect(children.first == .leaf(1))
        guard case let .split(.vertical, inner, _) = children.last else {
            Issue.record("expected the second half to be split vertically")
            return
        }
        #expect(inner == [.leaf(2), .leaf(3)])
        #expect(workbench.current?.panes == [1, 2, 3])
    }

    @Test("a pane that ends gives its space back and keeps the tab open")
    func closingOnePane() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "claude")
        workbench.split(with: 2, axis: .vertical)

        workbench.close(pane: 2)
        #expect(workbench.tabs.count == 1)
        #expect(workbench.current?.root == .leaf(1), "the split is gone with its second half")
        #expect(workbench.focus == 1, "the keyboard goes to what is left")
    }

    @Test("the last pane of a tab takes the tab with it")
    func closingTheLastPane() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "claude")
        workbench.open(pane: 2, title: "codex")

        workbench.close(pane: 2)
        #expect(workbench.tabs.count == 1)
        #expect(workbench.selected == workbench.tabs[0].id, "something is still shown")

        workbench.close(pane: 1)
        #expect(workbench.tabs.isEmpty)
        #expect(workbench.selected == nil)
    }

    @Test("closing a tab says which panes nobody is watching any more")
    func closingATab() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "claude")
        workbench.split(with: 2, axis: .horizontal)
        let id = workbench.selected!

        let released = workbench.closeTab(id)
        #expect(released.sorted() == [1, 2])
        #expect(workbench.tabs.isEmpty)
    }

    @Test("a divider moves only the two panes it is between")
    func movingADivider() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "a")
        workbench.split(with: 2, axis: .horizontal)
        workbench.split(with: 3, axis: .horizontal)
        let node = workbench.current!.root.id

        workbench.move(divider: 1, of: node, by: 0.1)
        guard case let .split(_, _, fractions) = workbench.current?.root else {
            Issue.record("expected a row")
            return
        }
        #expect(fractions[0] == 0.5, "the pane beyond the divider does not move")
        #expect(abs(fractions[1] - 0.35) < 0.000_1)
        #expect(abs(fractions[2] - 0.15) < 0.000_1)
    }

    @Test("a divider cannot be dragged past the end of a pane")
    func dividerLimits() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "a")
        workbench.split(with: 2, axis: .vertical)
        let node = workbench.current!.root.id

        workbench.move(divider: 0, of: node, by: 10)
        guard case let .split(_, _, fractions) = workbench.current?.root else {
            Issue.record("expected a split")
            return
        }
        #expect(fractions[1] >= 0.04, "the far pane keeps a sliver: \(fractions)")
        #expect(abs(fractions.reduce(0, +) - 1) < 0.000_1)
    }

    @Test("tabs step round, in both directions")
    func steppingTabs() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "one")
        workbench.open(pane: 2, title: "two")
        workbench.open(pane: 3, title: "three")

        workbench.step(by: 1)
        #expect(workbench.selected == workbench.tabs[0].id, "it wraps around")
        workbench.step(by: -1)
        #expect(workbench.selected == workbench.tabs[2].id)
    }

    @Test("a tab is renamed only when the name is new")
    func renaming() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "shell")
        let id = workbench.selected!

        workbench.rename(tab: id, to: "claude")
        #expect(workbench.current?.title == "claude")
        workbench.rename(tab: UUID(), to: "nobody")
        #expect(workbench.current?.title == "claude")
    }
}
