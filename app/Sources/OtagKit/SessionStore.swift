import Foundation
import Observation

/// One project, with the sessions running in it.
///
/// The daemon calls these workspaces; in the app they are what the sidebar
/// groups by, which is the folder the sessions are working in.
public struct WorkspaceGroup: Identifiable, Hashable, Sendable {
    public var name: String
    public var panes: [PaneInfo]

    public var id: String { name }

    /// What the group is doing, taken from the session that most wants the
    /// user — the same rule the terminal client's sidebar uses.
    public var status: PaneStatus {
        panes.map(\.status).max() ?? .idle
    }

    /// How many of its sessions are waiting for the user.
    public var waiting: Int {
        panes.filter { $0.status.wantsAttention }.count
    }
}

/// How the app's connection to the daemon stands.
public enum ConnectionState: Equatable, Sendable {
    case connecting
    case connected
    /// Nothing is connected, and this is why.
    case failed(String)
}

/// Everything the app knows about the daemon's session.
///
/// The daemon is the source of truth: this holds what it last said and
/// nothing it worked out for itself. One subscription brings every change,
/// so there is no polling anywhere in the app.
@MainActor
@Observable
public final class SessionStore {
    /// Every pane the daemon is running.
    public private(set) var panes: [PaneInfo] = []
    public private(set) var hasLoadedPanes = false
    /// How much of the agents' limits is gone.
    public private(set) var usage = UsageReport()
    /// What the agents are waiting for the user to answer.
    public private(set) var asks: [AskView] = []
    /// The readings behind the usage numbers, oldest first, for a timeline.
    public private(set) var history: [UsageSample] = []
    /// What otag did overnight, once it has been asked for.
    public private(set) var morningReport: [SessionReport]?
    /// The last thing otag did that is worth a line in the window: a
    /// handoff, or something the night shift sent.
    public private(set) var notice: String?
    /// How the connection stands.
    public private(set) var state: ConnectionState = .connecting
    /// Whether a re-read of the usage numbers is in flight, so the button
    /// that asked for it can show that something is happening.
    public private(set) var refreshingUsage = false
    /// When a usage report last arrived, for a word of feedback beside that
    /// button. Any report counts: one otag asked for, or one pushed because
    /// a session reported.
    public private(set) var usageUpdatedAt: Date?
    /// The daemon's version, once it has said.
    public private(set) var daemonVersion: String?

    /// The socket this store talks to.
    public let socket: URL

    /// How long the app waits for an answer to a re-read before it stops
    /// saying one is in flight. Shortened in tests.
    static let defaultUsageTimeout = Duration.seconds(5)
    var usageTimeout = SessionStore.defaultUsageTimeout

    private var connection: DaemonConnection?
    private var listening: Task<Void, Never>?
    private var refreshing: Task<Void, Never>?
    private var answerWaiting: Task<Void, Never>?
    private let startDaemon: Bool

    public init(socket: URL = Paths.socket(), startDaemon: Bool? = nil) {
        self.socket = socket
        // A custom socket belongs to an external daemon or a test fixture.
        // Never spawn the installed binary with the real HOME for that socket.
        self.startDaemon = startDaemon ?? (socket == Paths.socket())
    }

    /// The sessions grouped by project, in the order the daemon lists them.
    public var workspaces: [WorkspaceGroup] {
        var groups: [WorkspaceGroup] = []
        for pane in panes {
            let name = pane.workspace.isEmpty ? "otag" : pane.workspace
            if let index = groups.firstIndex(where: { $0.name == name }) {
                groups[index].panes.append(pane)
            } else {
                groups.append(WorkspaceGroup(name: name, panes: [pane]))
            }
        }
        return groups
    }

    /// The sessions in the order the sidebar lists them, with the projects
    /// flattened.
    ///
    /// What `⌘1`…`⌘9` count along, so the numbers in the rows and the
    /// shortcuts cannot disagree.
    public var ordered: [PaneID] {
        workspaces.flatMap { $0.panes.map(\.id) }
    }

    /// How many sessions are waiting for the user, for the Dock badge.
    public var waiting: Int {
        panes.filter { $0.status.wantsAttention }.count
    }

    /// One pane, by identifier.
    public func pane(_ id: PaneID) -> PaneInfo? {
        panes.first { $0.id == id }
    }

    /// The project a new session should start in, before the user says.
    ///
    /// Almost always the one they are looking at: a new session is nearly
    /// always another session in the project already in front of them, and
    /// making them find that folder in a file dialog again is work otag can
    /// do for them. The newest session's project is the fallback, and
    /// `nil` only when otag is running nothing at all.
    public func suggestedProject(near pane: PaneID?) -> String? {
        if let pane, let cwd = self.pane(pane)?.cwd, !cwd.isEmpty {
            return cwd
        }
        // The daemon lists panes oldest first, so the last one is the newest.
        return panes.reversed().compactMap(\.cwd).first { !$0.isEmpty }
    }

    /// Whether an agent is close enough to its limit that handing a session
    /// over is probably the next thing the user wants.
    ///
    /// The daemon works out the level, so this is the same 90% the night
    /// shift stops at and the same red the meter turns. It only makes the
    /// handoff button stand out; nothing is done automatically.
    public func isRunningOut(_ agent: AgentKind?) -> Bool {
        guard let agent else { return false }
        return usage.rows.contains { $0.agent == agent && $0.level == .red }
    }

    /// Answer what an agent is waiting for.
    ///
    /// The question leaves the list at once — it has been dealt with either
    /// way — and the daemon's answer says whether the agent took it. One it
    /// could not deliver is kept in `answerProblem`, because an answer that
    /// silently did nothing is the worst outcome there is.
    public func answer(_ ask: AskView, with answer: Answer) {
        asks.removeAll { $0.id == ask.id }
        answerProblem = nil
        send(.answer(ask: ask.id, answer: answer))
    }

    /// The last answer otag could not deliver, for the user to see.
    public var answerProblem: String?

    /// Forget the last failed answer.
    public func clearAnswerProblem() {
        answerProblem = nil
    }

    /// Forget the last notice.
    public func clearNotice() {
        notice = nil
        noticePane = nil
        noticeWaiting?.cancel()
        noticeWaiting = nil
    }

    /// How long a handoff's line stays up when the session it opened never
    /// appears. Shortened in tests.
    static let defaultNoticeTimeout = Duration.seconds(8)
    var noticeTimeout = SessionStore.defaultNoticeTimeout

    private var noticeWaiting: Task<Void, Never>?
    private var noticePane: PaneID?
    private var uiActive = true

    /// Say that a handoff happened, until the session it opened is there.
    ///
    /// "claude is taking over from codex, reading …" is worth reading once
    /// and then worth nothing: what it describes is the session now sitting
    /// in the window. So it goes as soon as that session appears, and after
    /// `noticeTimeout` either way, rather than staying across the rest of
    /// the day. The close button still works — this only means the user does
    /// not have to use it.
    private func note(handoff text: String, opened pane: PaneID) {
        clearNotice()
        guard !text.isEmpty else { return }
        guard !panes.contains(where: { $0.id == pane }) else { return }
        notice = text
        noticePane = pane
        noticeWaiting = Task { [weak self] in
            let waiting = self?.noticeTimeout ?? Self.defaultNoticeTimeout
            try? await Task.sleep(for: waiting)
            guard let self, !Task.isCancelled else { return }
            clearNotice()
        }
    }

    private func took(panes: [PaneInfo]) {
        if self.panes != panes { self.panes = panes }
        if !hasLoadedPanes { hasLoadedPanes = true }
        if let pane = noticePane, panes.contains(where: { $0.id == pane }) {
            clearNotice()
        }
    }

    /// The sessions a handoff has been asked for and not yet answered.
    ///
    /// A handoff writes a package and opens an agent, which takes a moment,
    /// and the button that starts one sits there the whole time. Asking
    /// twice opened two sessions on one task, so the button goes quiet until
    /// the daemon has answered. The daemon refuses a repeat as well — this is
    /// the half the user can see.
    public private(set) var handingOff: Set<PaneID> = []

    /// How long the app waits for a handoff before letting it be asked for
    /// again. Shortened in tests.
    static let defaultHandoffTimeout = Duration.seconds(30)
    var handoffTimeout = SessionStore.defaultHandoffTimeout

    private var handoffWaiting: Task<Void, Never>?

    /// Hand a session's work over, once.
    public func handOff(pane: PaneID, handoff: Handoff) {
        guard !handingOff.contains(pane) else { return }
        handingOff.insert(pane)
        send(.handoff(pane: pane, handoff: handoff))

        guard case .connected = state else {
            // Nothing was written, so no answer is coming.
            handingOff.remove(pane)
            return
        }

        // Nothing may wait forever: a daemon that never answers must not
        // leave the button dead for the rest of the session.
        handoffWaiting?.cancel()
        handoffWaiting = Task { [weak self] in
            let waiting = self?.handoffTimeout ?? Self.defaultHandoffTimeout
            try? await Task.sleep(for: waiting)
            guard let self, !Task.isCancelled else { return }
            handingOff.removeAll()
        }
    }

    /// A handoff has been answered, either way.
    ///
    /// The answer names the session that was *opened*, not the one handed
    /// over, so there is nothing to match on — and there is nothing to match
    /// either way, because a user hands one session over at a time.
    private func handoffSettled() {
        handingOff.removeAll()
        handoffWaiting?.cancel()
        handoffWaiting = nil
    }

    /// Ask otag what it did overnight.
    public func askForMorningReport() {
        send(.morningReport)
    }

    /// Ask the daemon for the usage numbers again, now.
    ///
    /// The daemon pushes a report whenever the numbers change, so this is
    /// not how the app normally learns them: it is for the refresh button
    /// and for the timer below, both of which exist because a reading can
    /// only ever be as fresh as the last session that reported one. There is
    /// no supported way to ask Claude Code for the account's limits without
    /// spending tokens, so otag asks the daemon, never the agent.
    /// How often the app asks for the numbers again, in seconds.
    static let usageRefresh = 30
    var usageRefreshInterval = Duration.seconds(SessionStore.usageRefresh)

    public func refreshUsage() {
        guard connection != nil else { return }
        refreshingUsage = true
        send(.usage)
        send(.usageHistory)

        guard case .connected = state else {
            // The request could not be written, so no answer is coming and
            // there is nothing to wait for.
            refreshingUsage = false
            return
        }

        // Nothing may wait forever. A daemon that does not answer within
        // this is a daemon with a problem of its own, and the connection
        // state is where that belongs.
        answerWaiting?.cancel()
        answerWaiting = Task { [weak self] in
            let waiting = self?.usageTimeout ?? Self.defaultUsageTimeout
            try? await Task.sleep(for: waiting)
            guard let self, !Task.isCancelled else { return }
            refreshingUsage = false
        }
    }

    /// Take in a usage report, however it arrived.
    private func took(_ report: UsageReport) {
        if usage != report {
            usage = report
            remember(report)
        }
        usageUpdatedAt = Date()
        refreshingUsage = false
        answerWaiting?.cancel()
        answerWaiting = nil
    }

    /// Forget the report, so the window can be opened fresh.
    public func clearMorningReport() {
        morningReport = nil
    }

    /// Connect to the daemon — starting it if it is not running — and keep
    /// the state up to date until `disconnect()`.
    public func start() {
        guard listening == nil else { return }
        listening = Task { [weak self] in
            await self?.run()
        }
        startUsageRefresh()
    }

    /// Pause periodic UI work while all main windows are hidden or minimized.
    /// The event subscription stays connected for inbox notifications.
    public func setUIActive(_ active: Bool) {
        guard uiActive != active else { return }
        uiActive = active
        if active {
            if case .connected = state { refreshUsage() }
            startUsageRefresh()
        } else {
            refreshing?.cancel()
            refreshing = nil
        }
    }

    private func startUsageRefresh() {
        guard uiActive, listening != nil, refreshing == nil else { return }
        refreshing = Task { [weak self] in
            await self?.keepUsageCurrent()
        }
    }

    /// Ask for the numbers again every half minute, while the app is open.
    ///
    /// The same interval the agents' status lines run on, so what the app
    /// shows is never more than one reporting round behind. The daemon's own
    /// copy changes only when a session reports, so this costs one small
    /// request and usually no redraw at all.
    private func keepUsageCurrent() async {
        while !Task.isCancelled {
            try? await Task.sleep(for: usageRefreshInterval)
            if Task.isCancelled { return }
            guard case .connected = state else { continue }
            refreshUsage()
        }
    }

    /// Stop listening and close the connection. The daemon keeps running.
    public func disconnect() {
        handoffSettled()
        noticeWaiting?.cancel()
        noticeWaiting = nil
        noticePane = nil
        listening?.cancel()
        listening = nil
        refreshing?.cancel()
        refreshing = nil
        answerWaiting?.cancel()
        answerWaiting = nil
        refreshingUsage = false
        connection?.close()
        connection = nil
    }

    /// Send one request, if there is a connection to send it on.
    public func send(_ request: Request) {
        guard let connection else { return }
        do {
            try connection.send(request)
        } catch {
            state = .failed(error.localizedDescription)
        }
    }

    /// Connect, subscribe, and apply everything that arrives; try again
    /// after a pause if the daemon goes away.
    private func run() async {
        while !Task.isCancelled {
            state = .connecting
            do {
                let connection = try await Daemon.connect(socket: socket, startIfNeeded: startDaemon)
                self.connection = connection
                try connection.send(.subscribe)
                try connection.send(.version)
                // The daemon keeps its readings across restarts, so the
                // timeline can be drawn at once rather than after an hour of
                // watching.
                try connection.send(.usageHistory)
                try connection.send(.inbox)
                state = .connected

                for await response in connection.responses {
                    if Task.isCancelled { break }
                    apply(response)
                }
                // The stream finished, which means the daemon closed the
                // connection or stopped.
                self.connection = nil
                if Task.isCancelled { return }
                state = .failed(DaemonError.disconnected.localizedDescription)
            } catch {
                connection = nil
                state = .failed(error.localizedDescription)
            }

            // A daemon that has just been restarted is worth waiting for,
            // but not worth hammering.
            try? await Task.sleep(for: .seconds(2))
        }
    }

    /// Take in one message from the daemon.
    ///
    /// Not private so that a test can hand the store the exact sequence of
    /// messages a real daemon sends; nothing outside the module calls it.
    func apply(_ response: Response) {
        switch response {
        case let .event(event):
            apply(event)
        case let .panes(panes):
            took(panes: panes)
        case let .usage(report):
            took(report)
        case let .inbox(asks):
            self.asks = asks
        case let .usageHistory(samples):
            history = samples
        case let .morningReport(sessions):
            morningReport = sessions
        case let .handoffDone(pane, _, summary):
            handoffSettled()
            note(handoff: summary, opened: pane)
        case let .version(version):
            daemonVersion = version
        case let .answered(ask, delivered, summary):
            asks.removeAll { $0.id == ask }
            if !delivered {
                answerProblem = summary.isEmpty
                    ? "otag could not answer that agent. Look at the session."
                    : summary
            }
        case let .error(message):
            handoffSettled()
            state = .failed(message)
        case .pong, .ok, .paneScreen, .output, .paneEnded, .sessionEnded,
             .sessionStarted, .teamUndoPreview, .unknown:
            // Pane bytes belong to whichever terminal view asked for them,
            // and the rest needs nothing from the store.
            break
        }
    }

    /// Keep a reading of each window, so the timeline grows while the app
    /// is open even though the daemon is the one that remembers them.
    private func remember(_ report: UsageReport) {
        let now = Int64(Date().timeIntervalSince1970)
        for row in report.rows {
            // A window otag has never had a reading of has nothing to put
            // on the timeline; the line starts when the first one arrives.
            guard let usedTenths = row.usedTenths else { continue }
            let sample = UsageSample(
                agent: row.agent,
                window: row.window,
                usedPercent: Double(usedTenths) / 10,
                resetsAt: row.resetsAt,
                at: now
            )
            // The daemon sends a report whenever anything about it changes,
            // which is not the same as a new reading of this window: a
            // percentage that has not moved is the same reading arriving
            // again, and the timeline already has it.
            let last = history.last {
                $0.agent == row.agent && $0.window == row.window
            }
            if last?.usedPercent == sample.usedPercent { continue }
            history.append(sample)
        }
    }

    /// Take in one event.
    private func apply(_ event: Event) {
        switch event {
        case let .panes(panes):
            took(panes: panes)
        case let .usage(report):
            took(report)
        case let .asked(ask):
            asks.removeAll { $0.pane == ask.pane }
            asks.append(ask)
        case let .resolved(ask, _):
            asks.removeAll { $0.id == ask }
        case let .handoff(pane, _, summary):
            note(handoff: summary, opened: pane)
        case let .notice(_, text):
            notice = text.isEmpty ? nil : text
        case .unknown:
            break
        }
    }
}
