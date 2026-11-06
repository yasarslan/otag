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
    public var undoStep: UInt64?
    public var violation: Bool
    public var revision: UInt64
    public var id: UInt64
    public var index: Int
    public var current: Bool
    public var waiting: Bool
    public var automatic: Bool
    public var manual: Bool
    enum CodingKeys: String, CodingKey {
        case id, index, current, waiting, automatic, manual, revision, violation
        case undoStep = "undo_step"
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(UInt64.self, forKey: .id)
        index = try c.decodeIfPresent(Int.self, forKey: .index) ?? 0
        revision = try c.decodeIfPresent(UInt64.self, forKey: .revision) ?? 0
        current = try c.decodeIfPresent(Bool.self, forKey: .current) ?? false
        waiting = try c.decodeIfPresent(Bool.self, forKey: .waiting) ?? true
        automatic = try c.decodeIfPresent(Bool.self, forKey: .automatic) ?? false
        manual = try c.decodeIfPresent(Bool.self, forKey: .manual) ?? false
        violation = try c.decodeIfPresent(Bool.self, forKey: .violation) ?? false
        undoStep = try c.decodeIfPresent(UInt64.self, forKey: .undoStep)
    }
}

/// Exact daemon-generated paths requiring confirmation before restoration.
public struct TeamUndoPreview: Codable, Sendable {
    public var team: UInt64
    public var pane: PaneID
    public var step: UInt64
    public var expected: String
    public var files: [String]
}
