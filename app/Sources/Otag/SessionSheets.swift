import OtagKit
import SwiftUI

/// The four questions the `⋯` menu can ask about a session.
///
/// One sheet, four bodies. They share a title, a Cancel and a default
/// button, which is the half of a dialog that is easy to get subtly wrong
/// four times.
struct SessionSheet: View {
    let actions: SessionActions
    let kind: SessionPrompt.Kind
    /// Every session the daemon is running, for `Split with…`.
    let everything: [PaneInfo]

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title)
                .font(.headline)
                .padding(.horizontal, 20)
                .padding(.top, 20)

            Group {
                switch kind {
                case .model: ModelBody(actions: actions, dismiss: { dismiss() })
                case .split: SplitBody(
                        actions: actions,
                        everything: everything,
                        dismiss: { dismiss() }
                    )
                case .rename: RenameBody(actions: actions, dismiss: { dismiss() })
                case .nightShift: NightShiftBody(actions: actions, dismiss: { dismiss() })
                }
            }
            .padding(20)
        }
        .frame(width: kind == .nightShift ? 440 : 400)
    }

    private var title: String {
        switch kind {
        case .model: return "Continue on another model"
        case .split: return "Put a session beside this one"
        case .rename: return "Rename this session"
        case .nightShift: return "Night shift"
        }
    }

    /// Cancel, and the one button that does the thing.
    private struct Buttons: View {
        let confirm: String
        var enabled = true
        let cancel: () -> Void
        let run: () -> Void

        var body: some View {
            HStack {
                Spacer()
                Button("Cancel", role: .cancel, action: cancel)
                    .keyboardShortcut(.cancelAction)
                Button(confirm, action: run)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!enabled)
            }
        }
    }

    /// Which model to continue this same session on.
    private struct ModelBody: View {
        let actions: SessionActions
        let dismiss: () -> Void
        @State private var model = ""

        var body: some View {
            VStack(alignment: .leading, spacing: 12) {
                Text("The conversation is kept; only the model changes. Leave this empty for "
                    + "the agent's fallback, where otag has a verified one.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                TextField("Model", text: $model, prompt: Text("sonnet"))
                    .textFieldStyle(.roundedBorder)
                Buttons(confirm: "Continue", cancel: dismiss) {
                    actions.continueOn(model: model)
                    dismiss()
                }
            }
        }
    }

    /// Which session to put beside this one.
    ///
    /// The discoverable half of side by side: dragging a row onto the edge
    /// of the window is faster, and nobody finds it without being told.
    private struct SplitBody: View {
        let actions: SessionActions
        let everything: [PaneInfo]
        let dismiss: () -> Void
        @State private var chosen: PaneID?
        @State private var axis = SplitAxis.horizontal

        var body: some View {
            VStack(alignment: .leading, spacing: 12) {
                if candidates.isEmpty {
                    Text("Every session otag is running is already on screen.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                } else {
                    Picker("Session", selection: $chosen) {
                        ForEach(candidates) { pane in
                            Text(actions.names.name(of: pane)).tag(PaneID?.some(pane.id))
                        }
                    }
                    .labelsHidden()

                    Picker("Where", selection: $axis) {
                        Text("Beside it").tag(SplitAxis.horizontal)
                        Text("Below it").tag(SplitAxis.vertical)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()

                    Text("otag shows up to \(Workbench.maximumPanes) sessions at once. You can "
                        + "also drag a session from the list onto the side or the bottom of "
                        + "this area.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }

                Buttons(confirm: "Split", enabled: chosen != nil, cancel: dismiss) {
                    guard let chosen else { return }
                    actions.split(with: chosen, axis: axis)
                    dismiss()
                }
            }
            .onAppear { chosen = candidates.first?.id }
        }

        private var candidates: [PaneInfo] {
            actions.splitCandidates(among: everything)
        }
    }

    /// What to call this session.
    private struct RenameBody: View {
        let actions: SessionActions
        let dismiss: () -> Void
        @State private var name = ""
        @FocusState private var focused: Bool

        var body: some View {
            VStack(alignment: .leading, spacing: 12) {
                TextField("Name", text: $name)
                    .textFieldStyle(.roundedBorder)
                    .focused($focused)
                    .onSubmit(save)
                Text("Leave this empty to go back to the name otag chose.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Buttons(confirm: "Rename", cancel: dismiss, run: save)
            }
            .onAppear {
                name = actions.names.isRenamed(actions.pane.id) ? actions.name : ""
                focused = true
            }
        }

        private func save() {
            actions.rename(to: name)
            dismiss()
        }
    }

    /// A queue of prompts, a threshold, and what to say when the window
    /// resets.
    ///
    /// otag does only what is written here, and only while the agent is idle
    /// and the window has room. Everything it does ends up in the morning
    /// report.
    private struct NightShiftBody: View {
        let actions: SessionActions
        let dismiss: () -> Void

        @State private var pauseAt: Double = 90
        @State private var resumePrompt = ""
        @State private var queueText = ""

        var body: some View {
            VStack(alignment: .leading, spacing: 12) {
                if let shift = actions.pane.nightShift {
                    Label(running(shift), systemImage: shift.paused ? "pause.circle" : "moon.stars")
                        .font(.callout)
                        .padding(8)
                        .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
                }

                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text("Pause at")
                        Spacer(minLength: 6)
                        Text("\(Int(pauseAt))%")
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                    }
                    .font(.callout)
                    Slider(value: $pauseAt, in: 50 ... 100, step: 5)
                }

                VStack(alignment: .leading, spacing: 4) {
                    Text("When a window resets, send")
                        .font(.callout)
                    TextField("nothing", text: $resumePrompt)
                        .textFieldStyle(.roundedBorder)
                }

                VStack(alignment: .leading, spacing: 4) {
                    Text("Queue · one prompt per line")
                        .font(.callout)
                    TextEditor(text: $queueText)
                        .font(.callout)
                        .frame(minHeight: 90)
                        .overlay {
                            RoundedRectangle(cornerRadius: 5).strokeBorder(.separator)
                        }
                }

                Text("otag sends the next prompt only while this session is idle and its "
                    + "window has room. It never types past a question.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                HStack {
                    if actions.isOnNightShift {
                        Button("Stop") {
                            actions.stopNightShift()
                            dismiss()
                        }
                    }
                    Spacer()
                    Button("Cancel", role: .cancel, action: dismiss)
                        .keyboardShortcut(.cancelAction)
                    Button(actions.isOnNightShift ? "Update" : "Start", action: start)
                        .keyboardShortcut(.defaultAction)
                        .disabled(prompts.isEmpty && resumePrompt.isEmpty)
                }
            }
            .onAppear {
                if let shift = actions.pane.nightShift {
                    pauseAt = Double(shift.pauseAt)
                    resumePrompt = shift.resumePrompt
                }
            }
        }

        private var prompts: [String] {
            queueText
                .split(separator: "\n")
                .map { $0.trimmingCharacters(in: .whitespaces) }
                .filter { !$0.isEmpty }
        }

        private func start() {
            actions.store.send(.nightShift(
                pane: actions.pane.id,
                plan: NightShiftPlan(
                    pauseAt: UInt16(pauseAt),
                    resumePrompt: resumePrompt,
                    queue: prompts
                )
            ))
            dismiss()
        }

        private func running(_ shift: NightShiftView) -> String {
            if shift.paused {
                return "Paused at \(shift.pauseAt)%, waiting for the window to reset"
            }
            if shift.queued > 0 {
                return "\(shift.sent) of \(shift.queued) sent"
            }
            return shift.sent > 0
                ? "\(shift.sent) sent"
                : "Waiting for the session to be idle"
        }
    }
}

/// What otag did while nobody was watching.
struct MorningReportView: View {
    let sessions: [SessionReport]
    let dismiss: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Morning report")
                    .font(.headline)
                Spacer()
                Button("Done", action: dismiss)
                    .keyboardShortcut(.defaultAction)
            }
            .padding(Chrome.gutter)
            Divider()

            if sessions.isEmpty {
                VStack(spacing: 6) {
                    Text("Nothing to report")
                        .font(.title3)
                    Text("otag reports what the night shift did. Nothing has been queued yet.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .padding(30)
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 20) {
                        ForEach(sessions) { session in
                            SessionReportView(session: session)
                        }
                    }
                    .padding(Chrome.gutter)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
        .frame(width: 560, height: 480)
    }
}

/// One session's night.
private struct SessionReportView: View {
    let session: SessionReport

    var body: some View {
        VStack(alignment: .leading, spacing: Chrome.gap) {
            HStack(spacing: 6) {
                AgentMark(agent: session.agent)
                Text(session.title.isEmpty
                    ? (session.agent?.label ?? "shell") : session.title)
                    .font(.subheadline.weight(.semibold))
                if !session.workspace.isEmpty {
                    Text(session.workspace)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
                Text(spent)
                    .font(.caption)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }

            Text(sent)
                .font(.caption)
                .foregroundStyle(.secondary)

            if !session.commits.isEmpty {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Commits")
                        .font(.caption.weight(.medium))
                    ForEach(session.commits, id: \.self) { commit in
                        Text(commit)
                            .font(.system(.caption, design: .monospaced))
                            .lineLimit(1)
                    }
                }
            }

            if !session.log.isEmpty {
                VStack(alignment: .leading, spacing: 2) {
                    Text("What otag did")
                        .font(.caption.weight(.medium))
                    ForEach(session.log) { entry in
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Text(entry.date, format: .dateTime.hour().minute())
                                .font(.system(.caption2, design: .monospaced))
                                .foregroundStyle(.secondary)
                            Text(entry.what)
                                .font(.caption2)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }
        }
        .padding(12)
        .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: Chrome.cardRadius))
    }

    private var sent: String {
        if session.remaining > 0 {
            return "\(session.sent) prompts sent · \(session.remaining) still queued"
        }
        return session.sent == 1 ? "1 prompt sent" : "\(session.sent) prompts sent"
    }

    private var spent: String {
        let total = session.tokens.input + session.tokens.output
            + session.tokens.cacheRead + session.tokens.cacheWritten
        return total == 0 ? "" : Readable.tokens(total) + " tokens"
    }
}
