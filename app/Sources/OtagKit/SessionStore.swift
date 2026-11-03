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
    /// The daemon's version, once it has said.
    public private(set) var daemonVersion: String?

    /// The socket this store talks to.
    public let socket: URL

    private var connection: DaemonConnection?
    private var listening: Task<Void, Never>?

    public init(socket: URL = Paths.socket()) {
        self.socket = socket
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

    /// How many sessions are waiting for the user, for the Dock badge.
    public var waiting: Int {
        panes.filter { $0.status.wantsAttention }.count
    }

    /// One pane, by identifier.
    public func pane(_ id: PaneID) -> PaneInfo? {
        panes.first { $0.id == id }
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
    }

    /// Ask otag what it did overnight.
    public func askForMorningReport() {
        send(.morningReport)
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
    }

    /// Stop listening and close the connection. The daemon keeps running.
    public func disconnect() {
        listening?.cancel()
        listening = nil
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
                let connection = try await Daemon.connect(socket: socket)
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
    private func apply(_ response: Response) {
        switch response {
        case let .event(event):
            apply(event)
        case let .panes(panes):
            self.panes = panes
        case let .usage(report):
            usage = report
        case let .inbox(asks):
            self.asks = asks
        case let .usageHistory(samples):
            history = samples
        case let .morningReport(sessions):
            morningReport = sessions
        case let .handoffDone(_, _, summary):
            notice = summary.isEmpty ? nil : summary
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
            state = .failed(message)
        case .pong, .ok, .paneScreen, .output, .paneEnded, .sessionEnded,
             .sessionStarted, .unknown:
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
            let sample = UsageSample(
                agent: row.agent,
                window: row.window,
                usedPercent: Double(row.usedTenths) / 10,
                resetsAt: nil,
                at: row.at == 0 ? now : row.at
            )
            // The daemon sends a report whenever anything about it changes,
            // which is not the same as a new reading of this window.
            if history.contains(where: { $0.id == sample.id }) { continue }
            history.append(sample)
        }
    }

    /// Take in one event.
    private func apply(_ event: Event) {
        switch event {
        case let .panes(panes):
            self.panes = panes
        case let .usage(report):
            usage = report
            remember(report)
        case let .asked(ask):
            asks.removeAll { $0.pane == ask.pane }
            asks.append(ask)
        case let .resolved(ask, _):
            asks.removeAll { $0.id == ask }
        case let .handoff(_, _, summary):
            notice = summary.isEmpty ? nil : summary
        case let .notice(_, text):
            notice = text.isEmpty ? nil : text
        case .unknown:
            break
        }
    }
}
