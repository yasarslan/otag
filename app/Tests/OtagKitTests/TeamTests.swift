import Foundation
import Testing
@testable import OtagKit

@Suite("Teams")
struct TeamTests {
    @Test func largeTeamsKeepEveryRoleInTheLayout() {
        #expect(Arrangement.team([1, 2, 3, 4, 5, 6, 7])?.panes == [1, 2, 3, 4, 5, 6, 7])
    }

    @Test func shellRequiresWriteTurn() {
        #expect(TeamRole(name: "Terminal", agent: nil).editable)
        #expect(!TeamRole(name: "Read").editable)
    }
    @Test func requestKeepsRoleOrderAndPermissions() throws {
        let request = Request.startTeam(cwd: "/project", roles: [
            TeamRole(name: "Plan"), TeamRole(name: "Coder", agent: .codex, canEdit: true),
            TeamRole(name: "Review"), TeamRole(name: "Extra", agent: nil)
        ], task: "Check the sample's test coverage.")
        let data = try JSONEncoder().encode(request)
        let json = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let roles = try #require(json["roles"] as? [[String: Any]])
        #expect(json["type"] as? String == "start_team")
        #expect(json["task"] as? String == "Check the sample's test coverage.")
        #expect(roles.map { $0["name"] as? String } == ["Plan", "Coder", "Review", "Extra"])
        #expect(roles[1]["can_edit"] as? Bool == true)
    }
    @Test func oldPanesHaveNoTeam() throws {
        let pane = try JSONDecoder().decode(PaneInfo.self, from: Data("{\"id\":1}".utf8))
        #expect(pane.team == nil)
    }
    @Test func roleNameTakesPrecedenceOverInjectedPrompt() throws {
        let pane = try JSONDecoder().decode(PaneInfo.self, from: Data("""
        {"id":1,"title":"Old title","first_prompt":"Role: Plan. Produce the task.",
         "team":{"id":1,"role_name":"Custom planning role"}}
        """.utf8))
        #expect(SessionName.of(pane: pane) == "Custom planning role")
        let legacy = try JSONDecoder().decode(PaneInfo.self, from: Data("""
        {"id":1,"title":"Plan","first_prompt":"Role: Plan. Produce the task.","team":{"id":1}}
        """.utf8))
        #expect(SessionName.of(pane: legacy) == "Plan")
    }

    @Test func advanceWarningCarriesConfirmationBackToDaemon() throws {
        let response = try Wire.decode(Data("""
        {"type":"team_advance_warning","pane":1,"revision":3,"back":false,
         "result":"QUESTION","confirmation":18446744073709551615}
        """.utf8))
        guard case let .teamAdvanceWarning(warning) = response else {
            Issue.record("expected a team warning")
            return
        }
        #expect(warning.result == "QUESTION")
        let request = Request.teamAdvance(pane: warning.pane, revision: warning.revision,
                                          back: warning.back, confirmation: warning.confirmation)
        let line = String(decoding: try Wire.encode(request), as: UTF8.self)
        #expect(line.contains("\"confirmation\":18446744073709551615"))
    }
    @Test func missingTeamFieldsRemainSafelyWaiting() throws {
        let value = try JSONDecoder().decode(TeamMembership.self, from: Data("{\"id\":1}".utf8))
        #expect(value.waiting)
        #expect(!value.automatic)
        #expect(value.undoStep == nil)
    }
    @Test func worktreeRolesRemainInOneOrderedTeam() throws {
        let data = Data("""
        [{"id":3,"cwd":"/project/worktree","workspace":"worktree","team":{"id":1,"index":1}},
         {"id":1,"cwd":"/project","workspace":"project","team":{"id":1,"index":0}},
         {"id":2,"cwd":"/other","workspace":"other"}]
        """.utf8)
        let panes = try JSONDecoder().decode([PaneInfo].self, from: data)
        #expect(TeamMembership.members(of: 1, in: panes).map(\.id) == [1, 3])
    }
}
