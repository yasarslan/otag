import Foundation
import Testing

@testable import OtagKit

/// Putting sessions side by side, and keeping them there.
@Suite("Sessions side by side")
@MainActor
struct SideBySideTests {
    @Test("a session dropped on an edge divides the whole tab")
    func droppingOnEachEdge() {
        for (edge, expected) in [
            (DropEdge.trailing, [1 as PaneID, 2]),
            (DropEdge.leading, [2, 1]),
            (DropEdge.bottom, [1, 2]),
            (DropEdge.top, [2, 1]),
        ] {
            let workbench = Workbench()
            workbench.open(pane: 1, title: "claude")
            workbench.place(pane: 2, on: edge, title: "codex")

            let tab = try! #require(workbench.current)
            #expect(tab.panes == expected, "\(edge) put them in the wrong order")
            #expect(tab.focus == 2, "the session just placed takes the keyboard")
            #expect(workbench.tabs.count == 1, "\(edge) made a tab instead of a split")
            if case let .split(axis, _, _) = tab.root {
                #expect(axis == edge.axis)
            } else {
                Issue.record("\(edge) did not split")
            }
        }
    }

    @Test("three sessions side by side")
    func threeAcross() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "one")
        workbench.place(pane: 2, on: .trailing, title: "two")
        workbench.place(pane: 3, on: .trailing, title: "three")

        let tab = try! #require(workbench.current)
        #expect(tab.panes == [1, 2, 3])
        #expect(workbench.tabs.count == 1)
    }

    @Test("a session already in the tab is moved, not shown twice")
    func draggingWithinATab() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "one")
        workbench.place(pane: 2, on: .trailing, title: "two")

        // The same session, dragged to the other side.
        workbench.place(pane: 2, on: .leading, title: "two")

        let tab = try! #require(workbench.current)
        #expect(tab.panes == [2, 1], "it moved")
        #expect(tab.panes.count == 2, "and is not in there twice")
    }

    @Test("a session dropped with nothing open becomes the first thing")
    func droppingIntoAnEmptyWindow() {
        let workbench = Workbench()
        workbench.place(pane: 1, on: .trailing, title: "one")

        #expect(workbench.tabs.count == 1)
        #expect(workbench.current?.panes == [1])
    }

    @Test("the focus steps through the panes of a tab, both ways")
    func steppingFocus() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "one")
        workbench.place(pane: 2, on: .trailing, title: "two")
        workbench.place(pane: 3, on: .trailing, title: "three")
        workbench.focus(pane: 1)

        workbench.stepFocus(by: 1)
        #expect(workbench.focus == 2)
        workbench.stepFocus(by: 1)
        #expect(workbench.focus == 3)
        workbench.stepFocus(by: 1)
        #expect(workbench.focus == 1, "it wraps around")
        workbench.stepFocus(by: -1)
        #expect(workbench.focus == 3, "and the other way too")
    }

    @Test("a tab with one session has nowhere to step to")
    func steppingWithOnePane() {
        let workbench = Workbench()
        workbench.open(pane: 1, title: "one")
        workbench.stepFocus(by: 1)
        #expect(workbench.focus == 1)
    }
}

/// The arrangement surviving a restart.
///
/// The daemon remembers the sessions; how they were arranged on screen is
/// the one thing the app owns, so it is the one thing the app has to write
/// down itself.
@Suite("The layout a restart puts back")
@MainActor
struct LayoutTests {
    @Test("three sessions side by side come back side by side")
    func aSplitSurvives() {
        let before = Workbench()
        before.open(pane: 1, title: "one")
        before.place(pane: 2, on: .trailing, title: "two")
        before.place(pane: 3, on: .bottom, title: "three")
        before.focus(pane: 2)

        let after = Workbench()
        after.restore(before.saved, alive: [1, 2, 3])

        #expect(after.tabs.count == 1, "one tab, as it was left")
        #expect(after.current?.panes == before.current?.panes)
        #expect(after.current?.root == before.current?.root, "including the arrangement")
        #expect(after.focus == 2, "and which of them had the keyboard")
    }

    @Test("the fractions a divider was dragged to come back")
    func draggedDividersSurvive() {
        let before = Workbench()
        before.open(pane: 1, title: "one")
        before.place(pane: 2, on: .trailing, title: "two")
        let root = try! #require(before.current?.root)
        before.move(divider: 0, of: root.id, by: 0.2)

        let after = Workbench()
        after.restore(before.saved, alive: [1, 2])

        guard case let .split(_, _, fractions) = try! #require(after.current?.root) else {
            Issue.record("the split did not come back")
            return
        }
        #expect(abs(fractions[0] - 0.7) < 0.001, "the divider is where it was left")
    }

    @Test("a session that is over is left out")
    func aSessionThatEndedIsDropped() {
        let before = Workbench()
        before.open(pane: 1, title: "one")
        before.place(pane: 2, on: .trailing, title: "two")
        before.place(pane: 3, on: .trailing, title: "three")

        // The daemon is no longer running session 2, which is the one in
        // the middle; the other two keep their places.
        let after = Workbench()
        after.restore(before.saved, alive: [1, 3])

        #expect(after.current?.panes == [1, 3])
        #expect(after.focus == 3, "and the one that had the keyboard still has it")
    }

    @Test("a focus on a session that is over lands on one that is not")
    func aFocusOnAGoneSession() {
        let before = Workbench()
        before.open(pane: 1, title: "one")
        before.place(pane: 2, on: .trailing, title: "two")
        #expect(before.focus == 2)

        // The session that had the keyboard is the one the daemon dropped.
        let after = Workbench()
        after.restore(before.saved, alive: [1])
        #expect(after.current?.panes == [1])
        #expect(after.focus == 1, "a tab has to be typeable into")
    }

    @Test("a tab whose sessions are all over does not come back at all")
    func anEmptyTabIsDropped() {
        let before = Workbench()
        before.open(pane: 1, title: "one")
        before.open(pane: 2, title: "two")

        let after = Workbench()
        after.restore(before.saved, alive: [2])

        #expect(after.tabs.count == 1)
        #expect(after.current?.panes == [2])
        #expect(after.selected == after.tabs.first?.id, "and something is showing")
    }

    @Test("nothing running means nothing comes back")
    func nothingAliveAtAll() {
        let before = Workbench()
        before.open(pane: 1, title: "one")

        let after = Workbench()
        after.restore(before.saved, alive: [])
        #expect(after.tabs.isEmpty)
        #expect(after.selected == nil)
    }

    @Test("a layout never replaces what the user is already looking at")
    func restoringOverSomethingOpen() {
        let before = Workbench()
        before.open(pane: 1, title: "one")

        let after = Workbench()
        after.open(pane: 9, title: "already here")
        after.restore(before.saved, alive: [1, 9])

        #expect(after.current?.panes == [9], "the open session stayed")
    }

    @Test("the layout survives being written down and read back")
    func throughJson() throws {
        let before = Workbench()
        before.open(pane: 1, title: "one")
        before.place(pane: 2, on: .bottom, title: "two")

        let data = try JSONEncoder().encode(before.saved)
        let layout = try JSONDecoder().decode(SavedLayout.self, from: data)

        let after = Workbench()
        after.restore(layout, alive: [1, 2])
        #expect(after.current?.root == before.current?.root)
        #expect(after.current?.title == "one")
    }

    @Test("a saved layout this build cannot read leaves the window empty")
    func unreadableLayout() {
        let defaults = try! #require(UserDefaults(suiteName: "otag-tests-\(UUID().uuidString)"))
        defaults.set(Data("from the future".utf8), forKey: Workbench.layoutKey)

        let workbench = Workbench()
        workbench.restore(from: defaults, alive: [1])
        #expect(workbench.tabs.isEmpty, "an unreadable layout is not an error")
    }

    @Test("what was saved is what is read back from the defaults")
    func savingAndLoading() throws {
        let defaults = try #require(UserDefaults(suiteName: "otag-tests-\(UUID().uuidString)"))
        let before = Workbench()
        before.open(pane: 1, title: "one")
        before.place(pane: 2, on: .trailing, title: "two")
        before.save(to: defaults)

        let after = Workbench()
        after.restore(from: defaults, alive: [1, 2])
        #expect(after.current?.panes == [1, 2])
    }
}

/// What a session dragged out of the sidebar carries.
@Suite("Dragging a session")
struct DragTests {
    @Test("a dragged session is recognised, and nothing else is")
    func onlyASessionIsADrop() {
        #expect(DraggedSession.pane(in: "otag-session:7") == 7)
        // A file path, a line of text, a URL: a drop otag did not start.
        #expect(DraggedSession.pane(in: "/Users/someone/notes.txt") == nil)
        #expect(DraggedSession.pane(in: "https://example.invalid") == nil)
        #expect(DraggedSession.pane(in: "otag-session:") == nil)
        #expect(DraggedSession.pane(in: "otag-session:not-a-number") == nil)
        #expect(DraggedSession.pane(in: "") == nil)
    }
}
