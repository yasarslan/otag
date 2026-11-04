import Foundation
import Testing

@testable import OtagKit

/// Handing a session over, exactly once.
///
/// A handoff writes a package and opens an agent, so the button that starts
/// one has to stop inviting a second click while the first is in flight. The
/// daemon refuses a repeat as well; this is the half the user can see.
@Suite("Handing a session over")
@MainActor
struct HandoffTests {
    @Test("a session being handed over cannot be handed over again")
    func askingTwiceSendsOnce() async throws {
        // A socket that accepts and never answers, which is exactly the
        // window the double click happened in: the request has gone out and
        // nothing has come back yet.
        let silent = try SilentSocket()
        defer { silent.stop() }

        let store = SessionStore(socket: silent.url)
        store.start()
        defer { store.disconnect() }
        try await until("the store connects") { store.state == .connected }

        let pane: PaneID = 1
        store.handOver(pane: pane, handoff: .agent(to: nil))
        #expect(store.handingOver.contains(pane), "the button has something to show at once")

        store.handOver(pane: pane, handoff: .agent(to: nil))
        #expect(
            store.handingOver == [pane],
            "the second ask changes nothing, so no second session is opened"
        )

        // Another session is another task, and is not held up by this one.
        store.handOver(pane: 2, handoff: .agent(to: nil))
        #expect(store.handingOver == [pane, 2])
    }

    @Test("a daemon that never answers does not leave the button dead")
    func aSilentDaemonTimesOut() async throws {
        let silent = try SilentSocket()
        defer { silent.stop() }

        let store = SessionStore(socket: silent.url)
        store.handoffTimeout = .milliseconds(300)
        store.start()
        defer { store.disconnect() }
        try await until("the store connects") { store.state == .connected }

        store.handOver(pane: 1, handoff: .agent(to: nil))
        #expect(store.handingOver.contains(1), "the request went out")

        try await until("the app to stop waiting") { store.handingOver.isEmpty }
    }

    @Test("a handoff with nothing connected is not left pending")
    func handingOverWithoutADaemon() {
        let store = SessionStore(
            socket: URL(fileURLWithPath: NSTemporaryDirectory() + "/otag-nowhere.sock")
        )
        store.handOver(pane: 1, handoff: .agent(to: nil))
        #expect(store.handingOver.isEmpty, "there is nobody to answer, so nothing is pending")
    }

    /// Wait for something to become true, or fail saying what did not.
    private func until(_ what: String, _ ready: () -> Bool) async throws {
        for _ in 0 ..< 100 {
            if ready() { return }
            try await Task.sleep(for: .milliseconds(100))
        }
        Issue.record("never saw \(what)")
    }
}

/// The line across the bottom of the window after a handoff.
///
/// "claude is taking over from codex, reading …" used to stay there for the
/// rest of the day. It describes the session now sitting in the window, so
/// it has nothing left to say once that session is on screen.
@Suite("The line a handoff leaves")
@MainActor
struct HandoffNoticeTests {
    @Test("the line goes once the session it opened is on screen")
    func theNoticeGoesWhenTheSessionArrives() async throws {
        let store = SessionStore(
            socket: URL(fileURLWithPath: NSTemporaryDirectory() + "/otag-nowhere.sock")
        )
        store.noticeTimeout = .seconds(30)

        store.apply(.handoffDone(pane: 7, package: nil, summary: "claude is taking over"))
        #expect(store.notice == "claude is taking over", "the user is told once")

        // The daemon sends the new pane a moment later, which is the session
        // the line was about.
        let opened = try JSONDecoder().decode(PaneInfo.self, from: Data(#"{"id":7}"#.utf8))
        store.apply(.panes([opened]))
        try await until("the line to go by itself") { store.notice == nil }
    }

    @Test("a line whose session never arrives still goes")
    func theNoticeTimesOut() async throws {
        let store = SessionStore(
            socket: URL(fileURLWithPath: NSTemporaryDirectory() + "/otag-nowhere.sock")
        )
        store.noticeTimeout = .milliseconds(300)

        store.apply(.handoffDone(pane: 7, package: nil, summary: "claude is taking over"))
        #expect(store.notice != nil)
        try await until("the line to time out") { store.notice == nil }
    }

    @Test("the close button still works, and stops the wait")
    func closingItByHandStillWorks() {
        let store = SessionStore(
            socket: URL(fileURLWithPath: NSTemporaryDirectory() + "/otag-nowhere.sock")
        )
        store.apply(.handoffDone(pane: 7, package: nil, summary: "claude is taking over"))
        store.clearNotice()
        #expect(store.notice == nil)
    }

    private func until(_ what: String, _ ready: () -> Bool) async throws {
        for _ in 0 ..< 100 {
            if ready() { return }
            try await Task.sleep(for: .milliseconds(100))
        }
        Issue.record("never saw \(what)")
    }
}

/// When otag draws attention to the handoff.
///
/// The button is always there; it only stands out when the agent in that
/// session is nearly out of room, which is when a handoff is the thing the
/// user probably wants next.
@Suite("An agent running out of room")
@MainActor
struct RunningOutTests {
    private func store(_ rows: [UsageRow]) -> SessionStore {
        let store = SessionStore(
            socket: URL(fileURLWithPath: NSTemporaryDirectory() + "/otag-nowhere.sock")
        )
        var report = UsageReport()
        report.rows = rows
        store.apply(.usage(report))
        return store
    }

    @Test("a window the daemon calls red is an agent running out")
    func redIsRunningOut() {
        let store = store([
            UsageRow(agent: .claude, window: .fiveHour, usedTenths: 950, level: .red),
            UsageRow(agent: .codex, window: .fiveHour, usedTenths: 120, level: .calm),
        ])
        #expect(store.isRunningOut(.claude))
        #expect(!store.isRunningOut(.codex))
        #expect(!store.isRunningOut(nil), "a shell has no limits to run out of")
    }

    @Test("a window otag has never read says nothing either way")
    func noReadingIsNotAWarning() {
        let store = store([
            UsageRow(agent: .claude, window: .fiveHour, usedTenths: nil),
        ])
        #expect(!store.isRunningOut(.claude))
    }

    @Test("amber is not yet running out")
    func amberIsNotRunningOut() {
        let store = store([
            UsageRow(agent: .claude, window: .weekly, usedTenths: 750, level: .amber),
        ])
        #expect(
            !store.isRunningOut(.claude),
            "seventy percent is worth a colour on the meter, not a highlighted button"
        )
    }
}
