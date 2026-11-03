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
    /// How large the model's context is, when otag knows. Without it a
    /// context meter has no denominator and only the count is shown.
    public var contextWindow: UInt64?
    /// What the night shift is doing for it, when it has been asked to.
    public var nightShift: NightShiftView?
    public var exit: PaneExit?

    enum CodingKeys: String, CodingKey {
        case id, size, workspace, title, agent, status, session, tokens, exit
        case contextWindow = "context_window"
        case nightShift = "night_shift"
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
        contextWindow = try container.decodeIfPresent(UInt64.self, forKey: .contextWindow)
        nightShift = try container.decodeIfPresent(NightShiftView.self, forKey: .nightShift)
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
    /// Tenths of a percentage point, as the daemon sends it, or nothing when
    /// the window has reset and no reading of the new one has arrived.
    public var usedTenths: UInt16?
    public var resetsIn: Int64?
    public var fullIn: Int64?
    public var level: UsageLevel = .calm
    public var at: Int64 = 0
    /// How old the reading is, in seconds, as the daemon measured it.
    public var age: Int64 = 0
    /// Whether the window has reset since the reading was taken, so that
    /// otag is waiting for fresh data rather than showing a percentage.
    public var reset: Bool = false
    /// The agent session the reading came from, when it said which.
    ///
    /// Usage is account-wide, so this can be a session otag is not running.
    public var session: String?

    public var id: String { "\(agent.rawValue)-\(window.rawValue)" }

    /// The percentage as a whole number, which is what is shown, or nothing
    /// when there is no current reading to show.
    public var usedPercent: Int? {
        usedTenths.map { Int($0 / 10) }
    }

    /// How much of the window is gone, as a fraction, for the meter.
    public var fraction: Double {
        usedTenths.map { Double($0) / 1_000 } ?? 0
    }

    /// Whether the reading is old enough that it should be shown as old.
    ///
    /// The daemon measures the age, so the app never compares the user's
    /// clock with the daemon's.
    public var isStale: Bool { age > 600 }

    /// Whether this window has a reading that describes it as it is now.
    ///
    /// A number nobody has confirmed for a while, and a window waiting for
    /// its first reading, are both shown as what they are rather than as
    /// the state of things.
    public var isCurrent: Bool { !reset && !isStale && usedTenths != nil }

    enum CodingKeys: String, CodingKey {
        case agent, window
        case usedTenths = "used_tenths"
        case resetsIn = "resets_in"
        case fullIn = "full_in"
        case level, at, age, reset, session
    }

    public init(
        agent: AgentKind,
        window: UsageWindow,
        usedTenths: UInt16?,
        resetsIn: Int64? = nil,
        fullIn: Int64? = nil,
        level: UsageLevel = .calm,
        at: Int64 = 0,
        age: Int64 = 0,
        reset: Bool = false,
        session: String? = nil
    ) {
        self.agent = agent
        self.window = window
        self.usedTenths = usedTenths
        self.resetsIn = resetsIn
        self.fullIn = fullIn
        self.level = level
        self.at = at
        self.age = age
        self.reset = reset
        self.session = session
    }

    /// Everything but the window itself may be missing, as from every other
    /// message: an older daemon sends no age and no reset at all.
    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        agent = try container.decode(AgentKind.self, forKey: .agent)
        window = try container.decode(UsageWindow.self, forKey: .window)
        usedTenths = try container.decodeIfPresent(UInt16.self, forKey: .usedTenths)
        resetsIn = try container.decodeIfPresent(Int64.self, forKey: .resetsIn)
        fullIn = try container.decodeIfPresent(Int64.self, forKey: .fullIn)
        level = (try? container.decodeIfPresent(UsageLevel.self, forKey: .level)) ?? .calm
        at = try container.decodeIfPresent(Int64.self, forKey: .at) ?? 0
        age = try container.decodeIfPresent(Int64.self, forKey: .age) ?? 0
        reset = try container.decodeIfPresent(Bool.self, forKey: .reset) ?? false
        session = try container.decodeIfPresent(String.self, forKey: .session)
    }
}

/// What otag counted itself, for an agent that reports no percentage.
public struct UsageCounted: Codable, Hashable, Sendable, Identifiable {
    public var agent: AgentKind
    public var tokens: UInt64

    public var id: String { agent.rawValue }
}

/// One reading behind the numbers, for a timeline.
public struct UsageSample: Codable, Hashable, Sendable, Identifiable {
    public var agent: AgentKind
    public var window: UsageWindow
    /// How much of the window was used, as a percentage.
    public var usedPercent: Double
    /// When the window resets, in seconds since the epoch.
    public var resetsAt: Int64?
    /// When the reading was taken, in seconds since the epoch.
    public var at: Int64

    public var id: String { "\(agent.rawValue)-\(window.rawValue)-\(at)" }

    /// When the reading was taken.
    public var date: Date { Date(timeIntervalSince1970: TimeInterval(at)) }

    enum CodingKeys: String, CodingKey {
        case agent, window, at
        case usedPercent = "used_pct"
        case resetsAt = "resets_at"
    }
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

    /// Whether any window has a reading that describes it as it is now.
    public var isFresh: Bool { rows.contains(where: \.isCurrent) }

    /// The newest reading in the report, which dates the whole of it.
    public var newest: UsageRow? {
        rows.min(by: { $0.age < $1.age })
    }

    /// How old the newest reading is, in seconds.
    public var age: Int64? { newest?.age }

    /// Where the numbers come from, in one line for a tooltip.
    ///
    /// Usage is account-wide: a reading can come from any Claude Code
    /// session on the machine, including one otag is not running. Saying
    /// which session it was, and how long ago, is what tells a current
    /// number from one nobody has refreshed.
    public func provenance(panes: [PaneInfo]) -> String {
        guard let newest else {
            return "No reading yet. A session reports the account's limits as it works."
        }

        let when = newest.age < 60 ? "just now" : Readable.ago(newest.age)
        var line = newest.age < 60 ? "Last reading just now" : "Last reading \(when)"
        if let session = newest.session {
            // A session otag is running can be named by what it is working
            // on, which means more to a person than an identifier.
            if let pane = panes.first(where: { $0.session == session }) {
                let what = pane.workspace.isEmpty ? pane.title : pane.workspace
                line += what.isEmpty ? "" : ", from the session in \(what)"
            } else {
                line += ", from session \(Readable.sessionLabel(session))"
            }
        }
        return line + ". Click to re-read."
    }
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

/// What the night shift should do for one session.
public struct NightShiftPlan: Codable, Hashable, Sendable {
    /// Percentage of a window at which to stop sending.
    public var pauseAt: UInt16?
    /// What to send when a window has reset and the queue is empty.
    public var resumePrompt: String?
    /// Prompts to work through, in order.
    public var queue: [String]

    public init(pauseAt: UInt16? = nil, resumePrompt: String? = nil, queue: [String] = []) {
        self.pauseAt = pauseAt
        self.resumePrompt = resumePrompt
        self.queue = queue
    }

    enum CodingKeys: String, CodingKey {
        case pauseAt = "pause_at"
        case resumePrompt = "resume_prompt"
        case queue
    }
}

/// What the night shift is doing for one session.
public struct NightShiftView: Codable, Hashable, Sendable {
    public var pauseAt: UInt16
    public var paused: Bool = false
    public var sent: Int = 0
    public var queued: Int = 0
    public var resumePrompt: String = ""

    /// How many prompts are still to go.
    public var remaining: Int { max(queued - sent, 0) }

    enum CodingKeys: String, CodingKey {
        case pauseAt = "pause_at"
        case paused, sent, queued
        case resumePrompt = "resume_prompt"
    }
}

/// One line of what otag did overnight.
public struct ReportEntry: Codable, Hashable, Sendable, Identifiable {
    public var at: Int64
    public var what: String

    public var id: String { "\(at)-\(what)" }
    public var date: Date { Date(timeIntervalSince1970: TimeInterval(at)) }
}

/// What otag did for one session while nobody was watching.
public struct SessionReport: Codable, Hashable, Sendable, Identifiable {
    public var pane: PaneID
    public var workspace: String = ""
    public var title: String = ""
    public var agent: AgentKind?
    public var tokens: Tokens = Tokens()
    public var sent: Int = 0
    public var remaining: Int = 0
    public var log: [ReportEntry] = []
    public var commits: [String] = []

    public var id: PaneID { pane }

    enum CodingKeys: String, CodingKey {
        case pane, workspace, title, agent, tokens, sent, remaining, log, commits
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        pane = try container.decode(PaneID.self, forKey: .pane)
        workspace = try container.decodeIfPresent(String.self, forKey: .workspace) ?? ""
        title = try container.decodeIfPresent(String.self, forKey: .title) ?? ""
        agent = try? container.decodeIfPresent(AgentKind.self, forKey: .agent)
        tokens = try container.decodeIfPresent(Tokens.self, forKey: .tokens) ?? Tokens()
        sent = try container.decodeIfPresent(Int.self, forKey: .sent) ?? 0
        remaining = try container.decodeIfPresent(Int.self, forKey: .remaining) ?? 0
        log = try container.decodeIfPresent([ReportEntry].self, forKey: .log) ?? []
        commits = try container.decodeIfPresent([String].self, forKey: .commits) ?? []
    }
}

/// What a handoff should do.
public enum Handoff: Sendable, Equatable {
    /// Open another agent on the same task, with a package to read.
    case agent(to: AgentKind?)
    /// Continue the same session on another model of the same agent.
    case model(String?)
}

/// What the new-session sheet asks for.
public struct StartSession: Sendable, Equatable {
    /// The project directory the session works in.
    public var cwd: String?
    /// The agent to run, or the user's shell when there is none.
    public var agent: AgentKind?
    /// A model to ask the agent for, when the user named one.
    public var model: String?
    /// What to ask the agent first.
    public var prompt: String?
    /// Whether the session gets a git worktree of its own.
    public var worktree: Bool

    public init(
        cwd: String? = nil,
        agent: AgentKind? = nil,
        model: String? = nil,
        prompt: String? = nil,
        worktree: Bool = false
    ) {
        self.cwd = cwd
        self.agent = agent
        self.model = model
        self.prompt = prompt
        self.worktree = worktree
    }
}

/// A command the app sends the daemon.
public enum Request: Sendable {
    case ping
    case version
    case listPanes
    case usage
    case subscribe
    case inbox
    case usageHistory
    case answer(ask: UInt64, answer: Answer)
    case startSession(StartSession)
    case handoff(pane: PaneID, handoff: Handoff)
    case nightShift(pane: PaneID, plan: NightShiftPlan?)
    case morningReport
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
        case .usageHistory:
            try container.encode("usage_history", forKey: Key("type"))
        case let .answer(ask, answer):
            try container.encode("answer", forKey: Key("type"))
            try container.encode(ask, forKey: Key("ask"))
            try container.encode(answer, forKey: Key("answer"))
        case let .startSession(asked):
            try container.encode("start_session", forKey: Key("type"))
            try container.encodeIfPresent(asked.cwd, forKey: Key("cwd"))
            try container.encodeIfPresent(asked.agent, forKey: Key("agent"))
            // An empty model or prompt is no model and no prompt: the daemon
            // should not be told to pass an empty argument along.
            try container.encodeIfPresent(asked.model?.isEmpty == true ? nil : asked.model,
                                          forKey: Key("model"))
            try container.encodeIfPresent(asked.prompt?.isEmpty == true ? nil : asked.prompt,
                                          forKey: Key("prompt"))
            if asked.worktree {
                try container.encode(true, forKey: Key("worktree"))
            }
        case let .handoff(pane, handoff):
            try container.encode("handoff", forKey: Key("type"))
            try container.encode(pane, forKey: Key("pane"))
            var mode = container.nestedContainer(keyedBy: Key.self, forKey: Key("handoff"))
            switch handoff {
            case let .agent(to):
                try mode.encode("agent", forKey: Key("mode"))
                try mode.encodeIfPresent(to, forKey: Key("to"))
            case let .model(model):
                try mode.encode("model", forKey: Key("mode"))
                try mode.encodeIfPresent(model?.isEmpty == true ? nil : model,
                                         forKey: Key("model"))
            }
        case let .nightShift(pane, plan):
            try container.encode("night_shift", forKey: Key("type"))
            try container.encode(pane, forKey: Key("pane"))
            try container.encodeIfPresent(plan, forKey: Key("plan"))
        case .morningReport:
            try container.encode("morning_report", forKey: Key("type"))
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
    case sessionStarted(pane: PaneID, workspace: String, cwd: String?)
    case paneScreen(pane: PaneID, size: PaneSize, bytes: Data)
    case output(pane: PaneID, bytes: Data)
    case paneEnded(pane: PaneID, exit: PaneExit?)
    case event(Event)
    case inbox([AskView])
    case usageHistory([UsageSample])
    case morningReport([SessionReport])
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
        case delivered, summary, package, message, view, frame, workspace, cwd, samples
        case sessions
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
        case "session_started":
            self = .sessionStarted(
                pane: try container.decode(PaneID.self, forKey: .pane),
                workspace: try container.decodeIfPresent(String.self, forKey: .workspace) ?? "",
                cwd: try container.decodeIfPresent(String.self, forKey: .cwd)
            )
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
        case "morning_report":
            self = .morningReport(
                try container.decodeIfPresent([SessionReport].self, forKey: .sessions) ?? []
            )
        case "usage_history":
            self = .usageHistory(
                try container.decodeIfPresent([UsageSample].self, forKey: .samples) ?? []
            )
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
