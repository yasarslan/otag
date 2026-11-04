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
