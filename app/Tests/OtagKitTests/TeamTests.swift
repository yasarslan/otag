import Foundation
import Testing
@testable import OtagKit

@Suite("Teams")
struct TeamTests {
    @Test func shellRequiresWriteTurn() {
        #expect(TeamRole(name: "Terminal", agent: nil).editable)
        #expect(!TeamRole(name: "Read").editable)
    }
    @Test func requestKeepsRoleOrderAndPermissions() throws {
        let request = Request.startTeam(cwd: "/project", roles: [
            TeamRole(name: "Plan"), TeamRole(name: "Coder", agent: .codex, canEdit: true),
            TeamRole(name: "Review"), TeamRole(name: "Extra", agent: nil)
        ])
        let data = try JSONEncoder().encode(request)
        let json = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let roles = try #require(json["roles"] as? [[String: Any]])
        #expect(json["type"] as? String == "start_team")
        #expect(roles.map { $0["name"] as? String } == ["Plan", "Coder", "Review", "Extra"])
        #expect(roles[1]["can_edit"] as? Bool == true)
    }
    @Test func oldPanesHaveNoTeam() throws {
        let pane = try JSONDecoder().decode(PaneInfo.self, from: Data("{\"id\":1}".utf8))
        #expect(pane.team == nil)
    }
}
