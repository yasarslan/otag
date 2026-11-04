import Foundation
import Testing

@testable import OtagKit

/// What the new-session sheet offers before the user says anything.
///
/// A new session is nearly always another session in the project already in
/// front of the user, so making them find that folder in a file dialog again
/// is work otag can do for them.
@Suite("The project a new session starts in")
@MainActor
struct NewSessionTests {
    /// A store holding the panes a daemon would have sent.
    private func store(_ panes: [(pane: PaneID, cwd: String?)]) throws -> SessionStore {
        let store = SessionStore(
            socket: URL(fileURLWithPath: NSTemporaryDirectory() + "/otag-nowhere.sock")
        )
        let decoded = try panes.map { pane in
            let cwd = pane.cwd.map { #""cwd":"\#($0)","# } ?? ""
            let json = #"{"id":\#(pane.pane),\#(cwd)"workspace":"app"}"#
            return try JSONDecoder().decode(PaneInfo.self, from: Data(json.utf8))
        }
        store.apply(.panes(decoded))
        return store
    }

    @Test("the project is the one the session on screen is working in")
    func theSelectedSessionsProject() throws {
        let store = try store([
            (pane: 1, cwd: "/projects/one"),
            (pane: 2, cwd: "/projects/two"),
        ])
        #expect(store.suggestedProject(near: 1) == "/projects/one")
        #expect(store.suggestedProject(near: 2) == "/projects/two")
    }

    @Test("with nothing on screen it is the newest session's project")
    func theNewestSessionsProject() throws {
        let store = try store([
            (pane: 1, cwd: "/projects/one"),
            (pane: 2, cwd: "/projects/two"),
        ])
        #expect(store.suggestedProject(near: nil) == "/projects/two")
    }

    @Test("a session whose directory otag does not know is skipped, not offered")
    func aSessionWithoutADirectory() throws {
        let store = try store([
            (pane: 1, cwd: "/projects/one"),
            (pane: 2, cwd: nil),
        ])
        // The newest knows no directory, so the one that does is offered
        // rather than an empty field.
        #expect(store.suggestedProject(near: nil) == "/projects/one")
        #expect(store.suggestedProject(near: 2) == "/projects/one")
        // And a pane the daemon is not running says nothing about itself.
        #expect(store.suggestedProject(near: 9) == "/projects/one")
    }

    @Test("with no sessions at all there is nothing to offer")
    func nothingRunning() throws {
        let store = try store([])
        #expect(store.suggestedProject(near: nil) == nil)
    }

    @Test("a pane carries the directory it is working in")
    func thePaneCarriesItsDirectory() throws {
        let json = #"{"id":1,"workspace":"app","cwd":"/projects/app"}"#
        let pane = try JSONDecoder().decode(PaneInfo.self, from: Data(json.utf8))
        #expect(pane.cwd == "/projects/app")

        // A daemon that does not send one is not an error: every field of
        // this is optional.
        let older = try JSONDecoder().decode(PaneInfo.self, from: Data(#"{"id":1}"#.utf8))
        #expect(older.cwd == nil)
    }
}
