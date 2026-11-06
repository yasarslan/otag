import Foundation

/// What a session is doing, in the four words the window uses.
///
/// The daemon reports six statuses; a person needs four. `looping` and
/// `error` both mean otag cannot carry on without the user, so both say
/// "Needs you" — and the word that tells them apart is one hover away, never
/// lost. A vocabulary of four is the whole point: a sidebar of six states is
/// a sidebar nobody reads.
public enum SessionBadge: String, Sendable, CaseIterable {
    case working
    case needsYou
    case done
    case idle

    /// The catalogue key for what the pill says.
    ///
    /// A key rather than a word: the pill is read by a person, so the word
    /// belongs in the `.strings` files with every other one.
    public var key: String {
        "status.\(rawValue)"
    }

    /// What the pill says, in the language otag is speaking.
    @MainActor
    public var label: String {
        Strings.t(key)
    }

    /// Whether this is the one thing in the window actually waiting for the
    /// user, which is what makes a row expand and the Dock badge count.
    public var wantsAttention: Bool {
        self == .needsYou
    }

    /// The badge for one status.
    public static func of(_ status: PaneStatus) -> SessionBadge {
        switch status {
        case .working: return .working
        case .needsApproval, .error, .looping: return .needsYou
        case .done: return .done
        case .idle: return .idle
        }
    }
}

public extension PaneStatus {
    /// What the pill says for this status.
    var badge: SessionBadge { SessionBadge.of(self) }

    /// The catalogue key for the exact word.
    ///
    /// "needs you" and "going in circles" and "error" all wear the same
    /// pill; this is where they are told apart.
    var preciseKey: String {
        switch self {
        case .looping: return "status.precise.looping"
        case .error: return "status.precise.error"
        default: return "status.precise.\(badge.rawValue)"
        }
    }

    /// The exact word, in the language otag is speaking.
    @MainActor
    var localizedPrecise: String {
        Strings.t(preciseKey)
    }

    /// Whether the pill understates this status, so the window should say
    /// more than the pill does.
    var needsTheLongerWord: Bool {
        self == .looping || self == .error
    }
}

/// Everything there is to do with one session, in one place.
///
/// The header bar's `⋯` menu and the sidebar's right click offer the same
/// list, in the same words, because they are this one type. Written as
/// methods rather than closures so that a test can call them: a menu item
/// that quietly stopped working is exactly the kind of thing nobody notices.
///
/// The two actions that need something from the user first — which model, and
/// which session to split with — are not here: they open a sheet, and a sheet
/// belongs to the window.
@MainActor
public struct SessionActions {
    public let store: SessionStore
    public let workbench: Workbench
    public let names: SessionNames
    public let pane: PaneInfo

    public init(
        store: SessionStore,
        workbench: Workbench,
        names: SessionNames,
        pane: PaneInfo
    ) {
        self.store = store
        self.workbench = workbench
        self.names = names
        self.pane = pane
    }

    /// What this session is called.
    public var name: String {
        names.name(of: pane)
    }

    /// The word the user gave it for the job it is doing.
    public var nickname: String {
        pane.nickname
    }

    /// How a popup lists this session: the nickname, the agent and the
    /// project, with whatever is missing left out rather than left blank.
    public var oneLine: String {
        [nickname, pane.agent.name, pane.workspace]
            .filter { !$0.isEmpty }
            .joined(separator: " · ")
    }

    /// Whether the limit guard is watching this session.
    public var isGuarded: Bool {
        pane.guardPlan != nil
    }

    /// Give this session a nickname, or clear it with an empty one.
    ///
    /// The daemon keeps it, which is why this is a request rather than a
    /// preference: a nickname has to survive a restart and reach every
    /// client.
    public func setNickname(_ nickname: String) {
        store.send(.setNickname(pane: pane.id, nickname: nickname))
    }

    /// Watch this session's limits, or stop watching.
    public func guardLimits(_ plan: GuardPlan?) {
        store.send(.guardLimits(pane: pane.id, plan: plan))
    }

    /// The one primary action, named after where the work would go — which
    /// is what the user is choosing between.
    public var handOffTitle: String {
        switch pane.agent {
        case .claude: return "Switch to Codex"
        case .codex, .deepseek: return "Switch to Claude"
        case nil: return "Switch agent"
        }
    }

    /// Whether this session can be handed anywhere at all.
    ///
    /// A shell has no agent to hand over from, and a handoff already in
    /// flight must not be asked for twice: that opened two agents on one
    /// task.
    public var canHandOff: Bool {
        pane.agent != nil && !isHandingOff
    }

    /// Whether a handoff for this session is already in flight.
    public var isHandingOff: Bool {
        store.handingOff.contains(pane.id)
    }

    /// Whether its agent is close enough to its limit that the handoff is
    /// worth drawing the eye to.
    public var isRunningOut: Bool {
        store.isRunningOut(pane.agent)
    }

    /// Whether the night shift is doing something for this session.
    public var isOnNightShift: Bool {
        pane.nightShift != nil
    }

    /// The sessions this one could be put beside: the ones the window is not
    /// already showing.
    public func splitCandidates(among panes: [PaneInfo]) -> [PaneInfo] {
        panes.filter { $0.id != pane.id && !workbench.isOpen($0.id) }
    }

    /// Hand this session's work to the other agent.
    public func handOff() {
        guard canHandOff else { return }
        store.handOff(pane: pane.id, handoff: .agent(to: nil))
    }

    /// Continue this same session on another model.
    public func continueOn(model: String) {
        guard pane.agent != nil else { return }
        store.handOff(
            pane: pane.id,
            handoff: .model(model.trimmingCharacters(in: .whitespaces))
        )
    }

    /// Put `other` beside this session.
    public func split(with other: PaneID, axis: SplitAxis = .horizontal) {
        workbench.focus(pane: pane.id)
        workbench.split(with: other, axis: axis)
    }

    /// Call this session something else.
    public func rename(to wanted: String) {
        names.rename(pane.id, to: wanted)
    }

    /// Stop the night shift for this session.
    public func stopNightShift() {
        guard isOnNightShift else { return }
        store.send(.nightShift(pane: pane.id, plan: nil))
    }

    /// Ask for what otag did overnight.
    public func askForMorningReport() {
        store.askForMorningReport()
    }

    /// Stop showing this session. The daemon keeps running it.
    public func close() {
        store.send(.detachPane(pane: pane.id))
        workbench.close(pane: pane.id)
    }
}
