import Foundation

/// A role requested by the new-team sheet; policy is enforced by the daemon.
public struct TeamRole: Codable, Equatable, Sendable {
    public var name: String
    public var agent: AgentKind?
    public var canEdit: Bool
    public var worktree: Bool
    enum CodingKeys: String, CodingKey {
        case name, agent, worktree
        case canEdit = "can_edit"
    }
    public init(name: String, agent: AgentKind? = .claude, canEdit: Bool = false, worktree: Bool = false) {
        self.name = name; self.agent = agent; self.canEdit = canEdit; self.worktree = worktree
    }
    public var editable: Bool { agent == nil || canEdit }
}

/// The daemon's presentation of one role in its team.
public struct TeamMembership: Codable, Hashable, Sendable {
    public var revision: UInt64
    public var id: UInt64
    public var index: Int
    public var current: Bool
    public var waiting: Bool
    public var automatic: Bool
    public var manual: Bool
}
