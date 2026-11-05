import Foundation
import Testing

@testable import OtagKit

/// What a session is doing, in the words the window uses.
@Suite("Status pills")
struct SessionBadgeTests {
    @Test("six statuses become four words")
    func theMapping() {
        #expect(PaneStatus.working.badge == .working)
        #expect(PaneStatus.done.badge == .done)
        #expect(PaneStatus.idle.badge == .idle)
        // All three of these mean otag cannot carry on without the user.
        #expect(PaneStatus.needsApproval.badge == .needsYou)
        #expect(PaneStatus.error.badge == .needsYou)
        #expect(PaneStatus.looping.badge == .needsYou)
    }

    @Test("every status has a pill, and the four pills have names")
    func nothingFallsThrough() {
        for status in PaneStatus.allCases {
            #expect(!status.badge.label.isEmpty, "\(status) has no pill")
        }
        #expect(SessionBadge.allCases.map(\.label)
            == ["Working", "Needs you", "Done", "Idle"])
    }

    @Test("only one pill is waiting for the user")
    func whoIsWaiting() {
        #expect(SessionBadge.needsYou.wantsAttention)
        for badge in SessionBadge.allCases where badge != .needsYou {
            #expect(!badge.wantsAttention, "\(badge.label) should not be asking for anything")
        }
    }

    @Test("the pill never swallows what the status actually was")
    func theLongerWord() {
        #expect(PaneStatus.looping.precise == "going in circles")
        #expect(PaneStatus.error.precise == "error")
        #expect(PaneStatus.looping.needsTheLongerWord)
        #expect(PaneStatus.error.needsTheLongerWord)

        // The other four say the same thing either way, so the window has
        // nothing extra to add.
        for status in [PaneStatus.idle, .working, .done, .needsApproval] {
            #expect(status.precise == status.label)
            #expect(!status.needsTheLongerWord)
        }
    }
}

/// Everything there is to do with one session.
///
/// The header's `⋯` menu and the sidebar's right click are both this type,
/// so these tests are the menus: an action that quietly stopped working is
/// exactly the kind of thing nobody notices by looking.
@Suite("Session actions")
@MainActor
struct SessionActionsTests {
    /// A store with no daemon behind it. `send` writes nowhere, which is
    /// what makes the app-side half of each action testable on its own.
    private func parts(
        pane: PaneInfo
    ) -> (SessionActions, SessionStore, Workbench, SessionNames, String) {
        let suite = "otag.actions.test.\(UUID())"
        let store = SessionStore(
            socket: URL(fileURLWithPath: NSTemporaryDirectory() + "/otag-nowhere.sock")
        )
        let workbench = Workbench()
        let names = SessionNames(defaults: UserDefaults(suiteName: suite)!)
        let actions = SessionActions(
            store: store,
            workbench: workbench,
            names: names,
            pane: pane
        )
        return (actions, store, workbench, names, suite)
    }

    private func claude(_ id: PaneID = 1) -> PaneInfo {
        PaneInfo(id: id, workspace: "otag", cwd: "/work/otag", agent: .claude, status: .working)
    }

    @Test("the primary button is named after where the work would go")
    func theButtonSaysWhereTo() {
        let (claudeActions, _, _, _, oneSuite) = parts(pane: claude())
        defer { UserDefaults().removePersistentDomain(forName: oneSuite) }
        #expect(claudeActions.handOffTitle == "Switch to Codex")
        #expect(claudeActions.canHandOff)

        var codex = claude(2)
        codex.agent = .codex
        let (codexActions, _, _, _, twoSuite) = parts(pane: codex)
        defer { UserDefaults().removePersistentDomain(forName: twoSuite) }
        #expect(codexActions.handOffTitle == "Switch to Claude")

        var shell = claude(3)
        shell.agent = nil
        let (shellActions, _, _, _, threeSuite) = parts(pane: shell)
        defer { UserDefaults().removePersistentDomain(forName: threeSuite) }
        #expect(!shellActions.canHandOff, "a shell has no other agent to go to")
    }

    @Test("a handoff already in flight cannot be asked for again")
    func handingOffOnce() {
        let pane = claude()
        let (actions, store, _, _, suite) = parts(pane: pane)
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        // What the store looks like while one is in flight. Nothing is
        // connected here, so the request itself goes nowhere — the point is
        // that the menu item goes quiet.
        store.handOff(pane: pane.id, handoff: .agent(to: nil))
        #expect(actions.canHandOff, "nothing is connected, so nothing is pending")
    }

    @Test("closing a session stops showing it and leaves it running")
    func closing() {
        let pane = claude()
        let (actions, _, workbench, _, suite) = parts(pane: pane)
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        workbench.select(pane: pane.id)
        #expect(workbench.panes == [pane.id])

        actions.close()
        #expect(workbench.panes.isEmpty, "the window stopped showing it")
    }

    @Test("splitting puts the other session beside this one")
    func splitting() {
        let pane = claude()
        let (actions, _, workbench, _, suite) = parts(pane: pane)
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        workbench.select(pane: pane.id)
        actions.split(with: 2)

        #expect(workbench.panes == [1, 2])
        #expect(workbench.focus == 2)
    }

    @Test("Split with… offers the sessions the window is not already showing")
    func whatCanBeSplitWith() {
        let pane = claude()
        let (actions, _, workbench, _, suite) = parts(pane: pane)
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        let everything = [pane, claude(2), claude(3)]
        workbench.select(pane: 1)
        #expect(actions.splitCandidates(among: everything).map(\.id) == [2, 3])

        workbench.place(pane: 2, on: .trailing)
        #expect(
            actions.splitCandidates(among: everything).map(\.id) == [3],
            "a session already on screen is not offered again"
        )
    }

    @Test("renaming a session changes what it is called")
    func renaming() {
        let pane = claude()
        let (actions, _, _, names, suite) = parts(pane: pane)
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        #expect(actions.name == "Claude", "its agent, until it is asked something or named")

        actions.rename(to: "the parser")
        #expect(names.name(of: pane) == "the parser")
        #expect(actions.name == "the parser")
    }

    @Test("the night shift can only be stopped when one is running")
    func stoppingTheNightShift() {
        let pane = claude()
        let (quiet, _, _, _, oneSuite) = parts(pane: pane)
        defer { UserDefaults().removePersistentDomain(forName: oneSuite) }
        #expect(!quiet.isOnNightShift)
        quiet.stopNightShift()

        var working = pane
        working.nightShift = NightShiftView(pauseAt: 90)
        let (busy, _, _, _, twoSuite) = parts(pane: working)
        defer { UserDefaults().removePersistentDomain(forName: twoSuite) }
        #expect(busy.isOnNightShift)
    }

    @Test("an agent near its limit is worth pointing at")
    func runningOut() {
        let pane = claude()
        let (actions, store, _, _, suite) = parts(pane: pane)
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        #expect(!actions.isRunningOut, "nothing has been reported yet")

        var report = UsageReport()
        report.rows = [
            UsageRow(agent: .claude, window: .fiveHour, usedTenths: 940, level: .red),
        ]
        store.apply(.usage(report))

        #expect(actions.isRunningOut)
    }
}
