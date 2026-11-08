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
    public var roleName: String?
    public var project: String
    public var undoStep: UInt64?
    public var violation: Bool
    public var revision: UInt64
    public var id: UInt64
    public var index: Int
    public var current: Bool
    public var waiting: Bool
    public var automatic: Bool
    public var needsYou: Bool
    public var nextRole: String?
    public var canAdvance: Bool
    public var manual: Bool
    enum CodingKeys: String, CodingKey {
        case id, index, current, waiting, automatic, manual, revision, violation, project
        case undoStep = "undo_step"
        case roleName = "role_name"
        case needsYou = "needs_you"
        case nextRole = "next_role"
        case canAdvance = "can_advance"
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(UInt64.self, forKey: .id)
        roleName = try c.decodeIfPresent(String.self, forKey: .roleName)
        project = try c.decodeIfPresent(String.self, forKey: .project) ?? ""
        index = try c.decodeIfPresent(Int.self, forKey: .index) ?? 0
        revision = try c.decodeIfPresent(UInt64.self, forKey: .revision) ?? 0
        current = try c.decodeIfPresent(Bool.self, forKey: .current) ?? false
        waiting = try c.decodeIfPresent(Bool.self, forKey: .waiting) ?? true
        automatic = try c.decodeIfPresent(Bool.self, forKey: .automatic) ?? false
        needsYou = try c.decodeIfPresent(Bool.self, forKey: .needsYou) ?? false
        nextRole = try c.decodeIfPresent(String.self, forKey: .nextRole)
        canAdvance = try c.decodeIfPresent(Bool.self, forKey: .canAdvance) ?? false
        manual = try c.decodeIfPresent(Bool.self, forKey: .manual) ?? false
        violation = try c.decodeIfPresent(Bool.self, forKey: .violation) ?? false
        undoStep = try c.decodeIfPresent(UInt64.self, forKey: .undoStep)
    }

    /// Group by daemon team identity across separate checkout directories.
    public static func members(of team: UInt64, in panes: [PaneInfo]) -> [PaneInfo] {
        panes.filter { $0.team?.id == team }.sorted { ($0.team?.index ?? 0) < ($1.team?.index ?? 0) }
    }
}

/// A daemon result that must be reviewed before advancing the team.
public struct TeamAdvanceWarning: Decodable, Sendable {
    public let pane: PaneID
    public let revision: UInt64
    public let back: Bool
    public let result: String
    public let confirmation: UInt64
}

/// Exact daemon-generated paths requiring confirmation before restoration.
public struct TeamUndoPreview: Codable, Sendable {
    public var team: UInt64
    public var pane: PaneID
    public var step: UInt64
    public var expected: String
    public var files: [String]
}
