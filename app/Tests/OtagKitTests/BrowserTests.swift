import Foundation
import Testing
@testable import OtagKit

@Suite("Browser sessions")
@MainActor
struct BrowserTests {
    @Test("ports and local hosts use HTTP; other hosts use HTTPS")
    func addresses() {
        for input in ["3000", " localhost:3000 ", "http://localhost:3000"] {
            #expect(BrowserSessions.address(input)?.absoluteString == "http://localhost:3000")
        }
        #expect(BrowserSessions.address("localhost:8080/path?q=1")?.absoluteString == "http://localhost:8080/path?q=1")
        #expect(BrowserSessions.address("127.0.0.1:8080")?.scheme == "http")
        #expect(BrowserSessions.address("example.com/path")?.absoluteString == "https://example.com/path")
        #expect(BrowserSessions.address("https://example.com")?.scheme == "https")
        for input in ["", "file:///etc/passwd", "javascript://alert(1)", "http://"] {
            #expect(BrowserSessions.address(input) == nil)
        }
    }

    @Test("each session saves its own URL and closing removes it across restart")
    func persistence() throws {
        let domain = "otag-browser-tests-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: domain))
        defer { defaults.removePersistentDomain(forName: domain) }
        let browsers = BrowserSessions(defaults: defaults)
        let first = browsers.create(project: "otag")
        let second = browsers.create(project: "otag")
        #expect(first != second && first > (1 << 63))
        #expect(browsers.session(first)?.url == BrowserSessions.defaultURL)
        let url = try #require(BrowserSessions.address("localhost:4321/test"))
        browsers.navigate(first, to: url)
        let restored = BrowserSessions(defaults: defaults)
        #expect(restored.session(first)?.url == url)
        #expect(restored.session(second)?.url == BrowserSessions.defaultURL)
        #expect(restored.session(first)?.project == "otag")
        restored.close(first)
        #expect(BrowserSessions(defaults: defaults).sessions.map(\.id) == [second])
    }

    @Test("browser and terminal sessions share arrange, drag splits and saved layouts")
    func layout() throws {
        let browser: PaneID = (1 << 63) + 1
        let workbench = Workbench()
        workbench.select(pane: 1)
        workbench.place(pane: browser, on: .trailing)
        #expect(workbench.panes == [1, browser])
        workbench.arrange([browser, 1, 2], keeping: browser)
        #expect(Set(workbench.panes) == Set([browser, 1, 2]))
        let layout = try JSONDecoder().decode(SavedLayout.self, from: JSONEncoder().encode(workbench.saved))
        let restored = Workbench()
        restored.restore(layout, alive: [browser, 1, 2])
        #expect(restored.panes == workbench.panes)
        #expect(restored.focus == browser)
    }
}
