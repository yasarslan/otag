import Foundation

/// The otag socket protocol, as the app needs it.
///
/// The daemon is the source of truth, so this file is a faithful translation
/// of its messages and nothing more: no state is derived here, and anything
/// the daemon sends that this build does not know about decodes to an
/// `unknown` case rather than failing. One JSON object per line, in both
/// directions.

// MARK: - Small values

/// Identifier of a pane. The daemon sends it as a plain number.
public typealias PaneID = UInt64

/// Size of a pane in character cells.
public struct PaneSize: Codable, Hashable, Sendable {
    public var rows: UInt16
    public var cols: UInt16

    public init(rows: UInt16, cols: UInt16) {
        self.rows = rows
        self.cols = cols
    }
}

/// What an agent in a pane is doing.
///
/// Ordered by how much it wants the user, like the daemon's own enum: a
/// workspace shows the largest status of its sessions.
public enum PaneStatus: String, Codable, Comparable, Sendable, CaseIterable {
    case idle
    case working
    case done
    case looping
    case error
    case needsApproval = "needs_approval"

    /// Whether this status is worth telling the user about.
    public var wantsAttention: Bool {
        switch self {
        case .needsApproval, .error, .looping: return true
        case .idle, .working, .done: return false
        }
    }

    /// How much the status wants the user, for `<`.
    private var weight: Int {
        switch self {
        case .idle: return 0
        case .working: return 1
        case .done: return 2
        case .looping: return 3
        case .error: return 4
        case .needsApproval: return 5
        }
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.weight < rhs.weight
    }

    /// What a status is called in the interface.
    public var label: String {
        switch self {
        case .idle: return "idle"
        case .working: return "working"
        case .done: return "done"
        case .looping: return "looping"
        case .error: return "error"
        case .needsApproval: return "needs you"
        }
    }
}

/// An agent otag has an adapter for.
public enum AgentKind: String, Codable, Sendable {
    case claude
    case codex

    /// What the agent is called in the interface.
    public var label: String {
        switch self {
        case .claude: return "Claude Code"
        case .codex: return "Codex"
        }
    }
}

/// How many tokens a session has used.
public struct Tokens: Codable, Hashable, Sendable {
    public var input: UInt64 = 0
    public var output: UInt64 = 0
    public var cacheRead: UInt64 = 0
    public var cacheWritten: UInt64 = 0
    public var context: UInt64 = 0

    enum CodingKeys: String, CodingKey {
        case input
        case output
        case cacheRead = "cache_read"
        case cacheWritten = "cache_written"
        case context
    }

    public init() {}
}

/// How a pane's program ended.
public struct PaneExit: Codable, Hashable, Sendable {
    public var code: UInt32?
    public var signal: String?
}

/// What the daemon reports about one pane.
public struct PaneInfo: Codable, Hashable, Sendable, Identifiable {
    public var id: PaneID
    public var size: PaneSize
    public var workspace: String = ""
    public var title: String = ""
    public var agent: AgentKind?
    public var status: PaneStatus = .idle
    public var session: String?
    public var tokens: Tokens = Tokens()
    public var exit: PaneExit?

    enum CodingKeys: String, CodingKey {
        case id, size, workspace, title, agent, status, session, tokens, exit
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(PaneID.self, forKey: .id)
        size = try container.decodeIfPresent(PaneSize.self, forKey: .size)
            ?? PaneSize(rows: 24, cols: 80)
        workspace = try container.decodeIfPresent(String.self, forKey: .workspace) ?? ""
        title = try container.decodeIfPresent(String.self, forKey: .title) ?? ""
        // An agent this build does not know about is no agent, not an error.
        agent = try? container.decodeIfPresent(AgentKind.self, forKey: .agent)
        status = (try? container.decodeIfPresent(PaneStatus.self, forKey: .status)) ?? .idle
        session = try container.decodeIfPresent(String.self, forKey: .session)
        tokens = try container.decodeIfPresent(Tokens.self, forKey: .tokens) ?? Tokens()
        exit = try container.decodeIfPresent(PaneExit.self, forKey: .exit)
    }
}

// MARK: - Usage

/// One of an agent's limit windows.
public enum UsageWindow: String, Codable, Sendable {
    case fiveHour = "five_hour"
    case weekly

    /// What the window is called, in as few characters as possible.
    public var label: String {
        switch self {
        case .fiveHour: return "5h"
        case .weekly: return "7d"
        }
    }
}

/// How worried the user should be about a window.
public enum UsageLevel: String, Codable, Sendable {
    case calm
    case amber
    case red
}

/// One window, as the widget shows it.
public struct UsageRow: Codable, Hashable, Sendable, Identifiable {
    public var agent: AgentKind
    public var window: UsageWindow
    /// Tenths of a percentage point, as the daemon sends it.
    public var usedTenths: UInt16
    public var resetsIn: Int64?
    public var fullIn: Int64?
    public var level: UsageLevel = .calm
    public var at: Int64 = 0

    public var id: String { "\(agent.rawValue)-\(window.rawValue)" }

    /// The percentage as a whole number, which is what is shown.
    public var usedPercent: Int { Int(usedTenths / 10) }

    enum CodingKeys: String, CodingKey {
        case agent, window
        case usedTenths = "used_tenths"
        case resetsIn = "resets_in"
        case fullIn = "full_in"
        case level, at
    }
}

/// What otag counted itself, for an agent that reports no percentage.
public struct UsageCounted: Codable, Hashable, Sendable, Identifiable {
    public var agent: AgentKind
    public var tokens: UInt64

    public var id: String { agent.rawValue }
}

/// How much of the agents' limits is gone.
public struct UsageReport: Codable, Hashable, Sendable {
    public var rows: [UsageRow] = []
    public var counted: [UsageCounted] = []

    public init() {}

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        rows = try container.decodeIfPresent([UsageRow].self, forKey: .rows) ?? []
        counted = try container.decodeIfPresent([UsageCounted].self, forKey: .counted) ?? []
    }

    enum CodingKeys: String, CodingKey {
        case rows, counted
    }

    public var isEmpty: Bool { rows.isEmpty && counted.isEmpty }
}

// MARK: - The inbox

/// What an agent is waiting for the user to answer.
public struct AskView: Codable, Hashable, Sendable, Identifiable {
    public var id: UInt64
    public var pane: PaneID
    public var agent: AgentKind?
    public var tool: String?
    public var summary: String = ""
    /// Seconds since the epoch.
    public var askedAt: UInt64?

    enum CodingKeys: String, CodingKey {
        case id, pane, agent, tool, summary
        case askedAt = "asked_at"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(UInt64.self, forKey: .id)
        pane = try container.decode(PaneID.self, forKey: .pane)
        agent = try? container.decodeIfPresent(AgentKind.self, forKey: .agent)
        tool = try container.decodeIfPresent(String.self, forKey: .tool)
        summary = try container.decodeIfPresent(String.self, forKey: .summary) ?? ""
        askedAt = try container.decodeIfPresent(UInt64.self, forKey: .askedAt)
    }
}

/// What the user answers a prompt with.
public enum Answer: String, Codable, Sendable {
    case allow
    case deny
}

// MARK: - Requests

/// A command the app sends the daemon.
public enum Request: Sendable {
    case ping
    case version
    case listPanes
    case usage
    case subscribe
    case inbox
    case answer(ask: UInt64, answer: Answer)
    case attachPane(pane: PaneID, size: PaneSize?)
    case detachPane(pane: PaneID)
    case paneInput(pane: PaneID, bytes: Data)
    case paneResize(pane: PaneID, size: PaneSize)
    case newWorkspace(cwd: String?)
    case detach
}

extension Request: Encodable {
    /// Keys built at runtime, because the protocol is an internally tagged
    /// enum: one object with a `type` and the variant's own fields.
    private struct Key: CodingKey {
        var stringValue: String
        var intValue: Int? { nil }
        init(_ value: String) { stringValue = value }
        init?(stringValue: String) { self.stringValue = stringValue }
        init?(intValue _: Int) { return nil }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: Key.self)
        switch self {
        case .ping:
            try container.encode("ping", forKey: Key("type"))
        case .version:
            try container.encode("version", forKey: Key("type"))
        case .listPanes:
            try container.encode("list_panes", forKey: Key("type"))
        case .usage:
            try container.encode("usage", forKey: Key("type"))
        case .subscribe:
            try container.encode("subscribe", forKey: Key("type"))
        case .inbox:
            try container.encode("inbox", forKey: Key("type"))
        case let .answer(ask, answer):
            try container.encode("answer", forKey: Key("type"))
            try container.encode(ask, forKey: Key("ask"))
            try container.encode(answer, forKey: Key("answer"))
        case let .attachPane(pane, size):
            try container.encode("attach_pane", forKey: Key("type"))
            try container.encode(pane, forKey: Key("pane"))
            try container.encodeIfPresent(size, forKey: Key("size"))
        case let .detachPane(pane):
            try container.encode("detach_pane", forKey: Key("type"))
            try container.encode(pane, forKey: Key("pane"))
        case let .paneInput(pane, bytes):
            try container.encode("pane_input", forKey: Key("type"))
            try container.encode(pane, forKey: Key("pane"))
            // The daemon reads bytes as base64, which is what `Data`
            // encodes to.
            try container.encode(bytes, forKey: Key("bytes"))
        case let .paneResize(pane, size):
            try container.encode("pane_resize", forKey: Key("type"))
            try container.encode(pane, forKey: Key("pane"))
            try container.encode(size, forKey: Key("size"))
        case let .newWorkspace(cwd):
            try container.encode("new_workspace", forKey: Key("type"))
            try container.encodeIfPresent(cwd, forKey: Key("cwd"))
        case .detach:
            try container.encode("detach", forKey: Key("type"))
        }
    }
}

// MARK: - Responses

/// Something the daemon tells a subscribed client about.
public enum Event: Sendable {
    case panes([PaneInfo])
    case usage(UsageReport)
    case asked(AskView)
    case resolved(ask: UInt64, answer: Answer?)
    case handoff(pane: PaneID, package: String?, summary: String)
    case notice(pane: PaneID?, text: String)
    /// An event this build does not know about.
    case unknown(String)
}

extension Event: Decodable {
    private enum Key: String, CodingKey {
        case event, panes, report, ask, answer, pane, package, summary, text
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: Key.self)
        let kind = (try? container.decode(String.self, forKey: .event)) ?? ""
        switch kind {
        case "panes":
            self = .panes(try container.decodeIfPresent([PaneInfo].self, forKey: .panes) ?? [])
        case "usage":
            self = .usage(try container.decodeIfPresent(UsageReport.self, forKey: .report)
                ?? UsageReport())
        case "asked":
            self = .asked(try container.decode(AskView.self, forKey: .ask))
        case "resolved":
            self = .resolved(
                ask: try container.decode(UInt64.self, forKey: .ask),
                answer: try? container.decodeIfPresent(Answer.self, forKey: .answer)
            )
        case "handoff":
            self = .handoff(
                pane: try container.decode(PaneID.self, forKey: .pane),
                package: try container.decodeIfPresent(String.self, forKey: .package),
                summary: try container.decodeIfPresent(String.self, forKey: .summary) ?? ""
            )
        case "notice":
            self = .notice(
                pane: try container.decodeIfPresent(PaneID.self, forKey: .pane),
                text: try container.decodeIfPresent(String.self, forKey: .text) ?? ""
            )
        default:
            self = .unknown(kind)
        }
    }
}

/// The daemon's answer, or something it pushed.
public enum Response: Sendable {
    case pong
    case ok
    case version(String)
    case panes([PaneInfo])
    case usage(UsageReport)
    case paneScreen(pane: PaneID, size: PaneSize, bytes: Data)
    case output(pane: PaneID, bytes: Data)
    case paneEnded(pane: PaneID, exit: PaneExit?)
    case event(Event)
    case inbox([AskView])
    case answered(ask: UInt64, delivered: Bool, summary: String)
    case handoffDone(pane: PaneID, package: String?, summary: String)
    case sessionEnded(exit: PaneExit?)
    case error(String)
    /// A message this build does not know about, named by its `type`.
    case unknown(String)
}

extension Response: Decodable {
    private enum Key: String, CodingKey {
        case type, version, panes, report, pane, size, bytes, exit, event, asks, ask
        case delivered, summary, package, message, view, frame
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: Key.self)
        let kind = (try? container.decode(String.self, forKey: .type)) ?? ""
        switch kind {
        case "pong":
            self = .pong
        case "ok":
            self = .ok
        case "version":
            self = .version(try container.decodeIfPresent(String.self, forKey: .version) ?? "")
        case "panes":
            self = .panes(try container.decodeIfPresent([PaneInfo].self, forKey: .panes) ?? [])
        case "usage":
            self = .usage(try container.decodeIfPresent(UsageReport.self, forKey: .report)
                ?? UsageReport())
        case "pane_screen":
            self = .paneScreen(
                pane: try container.decode(PaneID.self, forKey: .pane),
                size: try container.decode(PaneSize.self, forKey: .size),
                bytes: try container.decodeIfPresent(Data.self, forKey: .bytes) ?? Data()
            )
        case "output":
            self = .output(
                pane: try container.decode(PaneID.self, forKey: .pane),
                bytes: try container.decodeIfPresent(Data.self, forKey: .bytes) ?? Data()
            )
        case "pane_ended":
            self = .paneEnded(
                pane: try container.decode(PaneID.self, forKey: .pane),
                exit: try container.decodeIfPresent(PaneExit.self, forKey: .exit)
            )
        case "event":
            self = .event(try container.decode(Event.self, forKey: .event))
        case "inbox":
            self = .inbox(try container.decodeIfPresent([AskView].self, forKey: .asks) ?? [])
        case "answered":
            self = .answered(
                ask: try container.decode(UInt64.self, forKey: .ask),
                delivered: try container.decodeIfPresent(Bool.self, forKey: .delivered) ?? false,
                summary: try container.decodeIfPresent(String.self, forKey: .summary) ?? ""
            )
        case "handoff_done":
            self = .handoffDone(
                pane: try container.decode(PaneID.self, forKey: .pane),
                package: try container.decodeIfPresent(String.self, forKey: .package),
                summary: try container.decodeIfPresent(String.self, forKey: .summary) ?? ""
            )
        case "session_ended":
            self = .sessionEnded(exit: try container.decodeIfPresent(PaneExit.self, forKey: .exit))
        case "error":
            self = .error(try container.decodeIfPresent(String.self, forKey: .message) ?? "")
        default:
            self = .unknown(kind)
        }
    }
}

// MARK: - Coding

/// How the app reads and writes the daemon's lines.
public enum Wire {
    /// One request as a line of JSON, newline included.
    public static func encode(_ request: Request) throws -> Data {
        let encoder = JSONEncoder()
        // The daemon reads bytes as base64 and so does everything else here.
        encoder.dataEncodingStrategy = .base64
        var line = try encoder.encode(request)
        line.append(0x0A)
        return line
    }

    /// One line of JSON as a response.
    public static func decode(_ line: Data) throws -> Response {
        let decoder = JSONDecoder()
        decoder.dataDecodingStrategy = .base64
        return try decoder.decode(Response.self, from: line)
    }
}
