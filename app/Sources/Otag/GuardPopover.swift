import OtagKit
import SwiftUI

/// The limit guard: which sessions to watch, at what share of their limits,
/// and what to do when they get there.
///
/// The daemon does the watching, which is the point — it keeps working with
/// this window closed. This is only where the user says what they want.
struct GuardPopover: View {
    let store: SessionStore
    let names: SessionNames
    let workbench: Workbench

    /// The sessions the user has ticked in this popover.
    @State private var chosen: Set<PaneID> = []
    @State private var threshold: Double = 90
    @State private var whenReached: GuardAction = .wait
    @State private var prompt = GuardPopover.defaultPrompt
    @State private var filled = false

    /// What otag types when a window resets, unless the user changes it.
    ///
    /// The same sentence the daemon falls back to; it is written here too so
    /// the field has something in it before anything has been guarded.
    static var defaultPrompt: String { Strings.t("guard.resume.default") }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Chrome.gutter) {
                Text(Strings.t("guard.title"))
                    .font(.headline)

                // The one thing a person has to know before choosing, said
                // once and in plain words rather than hidden in a tooltip.
                Text(.init(Strings.t("guard.promise")))
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                if guardable.isEmpty {
                    Text(Strings.t("guard.none"))
                        .font(.callout)
                        .foregroundStyle(.secondary)
                } else {
                    sessions
                    Divider()
                    thresholdSlider
                    Divider()
                    choices
                    Divider()
                    buttons
                }
            }
            .padding(Chrome.gutter)
            .frame(width: 400, alignment: .leading)
        }
        .frame(maxHeight: 620)
        .onAppear(perform: fill)
    }

    /// Which sessions to watch. Two agents running at once can each be
    /// guarded, with their own threshold and their own choice.
    private var sessions: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(Strings.t("guard.sessions"))
                .font(.subheadline.weight(.semibold))
            ForEach(guardable) { pane in
                SessionTick(
                    pane: pane,
                    actions: actions(for: pane),
                    ticked: chosen.contains(pane.id)
                ) { on in
                    if on { chosen.insert(pane.id) } else { chosen.remove(pane.id) }
                }
            }
        }
    }

    private var thresholdSlider: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(Strings.t("guard.threshold"))
                    .font(.subheadline.weight(.semibold))
                Spacer(minLength: 6)
                Text("\(Int(threshold))%")
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
            Slider(value: $threshold, in: 50 ... 100, step: 5)
            Text(Strings.t("guard.threshold.detail"))
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var choices: some View {
        VStack(alignment: .leading, spacing: Chrome.gap) {
            Text(Strings.t("guard.when"))
                .font(.subheadline.weight(.semibold))

            Picker("", selection: $whenReached) {
                Text(Strings.t("guard.when.wait")).tag(GuardAction.wait)
                Text(Strings.t("guard.when.resume")).tag(GuardAction.resume)
                Text(Strings.t("guard.when.switch")).tag(GuardAction.switchAgent)
            }
            .pickerStyle(.radioGroup)
            .labelsHidden()

            switch whenReached {
            case .wait:
                note(Strings.t("guard.when.wait.detail"))
            case .resume:
                VStack(alignment: .leading, spacing: 4) {
                    note(Strings.t("guard.when.resume.detail"))
                    TextField("", text: $prompt, axis: .vertical)
                        .textFieldStyle(.roundedBorder)
                        .lineLimit(2 ... 4)
                }
            case .switchAgent:
                note(Strings.t("guard.when.switch.detail"))
            }
        }
    }

    private var buttons: some View {
        HStack {
            if chosen.contains(where: { isGuarded($0) }) {
                Button(Strings.t("guard.stop")) {
                    for pane in chosen {
                        store.send(.guardLimits(pane: pane, plan: nil))
                    }
                }
            }
            Spacer()
            Button(Strings.t("guard.start")) {
                let plan = GuardPlan(
                    threshold: UInt16(threshold),
                    whenReached: whenReached,
                    prompt: prompt
                )
                for pane in chosen {
                    store.send(.guardLimits(pane: pane, plan: plan))
                }
            }
            .keyboardShortcut(.defaultAction)
            .disabled(chosen.isEmpty)
        }
    }

    private func note(_ text: String) -> some View {
        Text(.init(text))
            .font(.caption)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }

    /// Only a session running an agent has limits to guard.
    private var guardable: [PaneInfo] {
        store.panes.filter { $0.agent != nil }
    }

    private func isGuarded(_ pane: PaneID) -> Bool {
        store.pane(pane)?.guardPlan != nil
    }

    private func actions(for pane: PaneInfo) -> SessionActions {
        SessionActions(store: store, workbench: workbench, names: names, pane: pane)
    }

    /// Open with what is already set, so the popover shows the truth rather
    /// than otag's defaults over the top of it.
    ///
    /// The session with the keyboard is ticked when nothing is guarded yet,
    /// because that is nearly always the one the user came here for.
    private func fill() {
        guard !filled else { return }
        filled = true

        let guarded = guardable.filter { $0.guardPlan != nil }
        if let first = guarded.first?.guardPlan {
            threshold = Double(first.threshold)
            whenReached = first.whenReached
            if !first.prompt.isEmpty { prompt = first.prompt }
        }
        chosen = guarded.isEmpty
            ? Set(workbench.focus.map { [$0] } ?? [])
            : Set(guarded.map(\.id))
    }
}

/// One session in the guard's list: what it is, what it is using, and what
/// the guard is doing about it.
private struct SessionTick: View {
    let pane: PaneInfo
    let actions: SessionActions
    let ticked: Bool
    let tick: (Bool) -> Void

    var body: some View {
        Toggle(isOn: Binding(get: { ticked }, set: tick)) {
            HStack(spacing: 6) {
                AgentMark(agent: pane.agent, size: 7)
                VStack(alignment: .leading, spacing: 1) {
                    Text(label)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Text(state)
                        .font(.caption)
                        .foregroundStyle(stateColour)
                        .lineLimit(1)
                }
            }
        }
        .toggleStyle(.checkbox)
    }

    /// `nickname · agent · project`, with what is missing left out.
    private var label: String {
        actions.oneLine.isEmpty ? actions.name : actions.oneLine
    }

    /// What the guard is doing for this session, in a few words.
    private var state: String {
        guard let plan = pane.guardPlan else {
            return Strings.t("guard.state.unguarded")
        }
        if plan.handedOff {
            return Strings.t("guard.state.handed", Int(plan.used))
        }
        if plan.paused {
            guard let resets = Readable.resetClock(plan.resetsAt) else {
                return Strings.t("guard.state.stopped", Int(plan.used))
            }
            return Strings.t("guard.state.stopped.until", Int(plan.used), resets)
        }
        return Strings.t("guard.state.guarding", Int(plan.used), Int(plan.threshold))
    }

    private var stateColour: Color {
        guard let plan = pane.guardPlan else { return .secondary }
        return plan.paused || plan.handedOff ? .orange : .secondary
    }
}
