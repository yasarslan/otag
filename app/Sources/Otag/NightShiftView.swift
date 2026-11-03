import OtagKit
import SwiftUI

/// The night shift for one session: a queue of prompts, a threshold, and
/// what to say when the window resets.
///
/// otag does only what is written here, and only while the agent is idle and
/// the window has room. Everything it does ends up in the morning report.
struct NightShiftPanel: View {
    let store: SessionStore
    let pane: PaneInfo

    @State private var pauseAt: Double = 90
    @State private var resumePrompt = ""
    @State private var queueText = ""
    @State private var loaded: PaneID?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let shift = pane.nightShift {
                Running(shift: shift)
            }

            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text("Pause at")
                    Spacer(minLength: 6)
                    Text("\(Int(pauseAt))%")
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                }
                .font(.caption)
                Slider(value: $pauseAt, in: 50 ... 100, step: 5)
            }

            VStack(alignment: .leading, spacing: 4) {
                Text("When a window resets, send")
                    .font(.caption)
                TextField("nothing", text: $resumePrompt)
                    .textFieldStyle(.roundedBorder)
                    .font(.caption)
            }

            VStack(alignment: .leading, spacing: 4) {
                Text("Queue · one prompt per line")
                    .font(.caption)
                TextEditor(text: $queueText)
                    .font(.caption)
                    .frame(minHeight: 80)
                    .overlay {
                        RoundedRectangle(cornerRadius: 5).strokeBorder(.separator)
                    }
            }

            HStack(spacing: 8) {
                Button(pane.nightShift == nil ? "Start" : "Update") { apply() }
                    .controlSize(.small)
                    .disabled(prompts.isEmpty && resumePrompt.isEmpty)
                if pane.nightShift != nil {
                    Button("Stop") { store.send(.nightShift(pane: pane.id, plan: nil)) }
                        .controlSize(.small)
                }
                Spacer(minLength: 0)
            }

            Text("otag sends the next prompt only while this session is idle and its window "
                + "has room. It never types past a question.")
                .font(.caption2)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .onAppear { load() }
        .onChange(of: pane.id) { _, _ in load() }
    }

    /// The prompts, as the queue editor has them.
    private var prompts: [String] {
        queueText
            .split(separator: "\n")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    /// Fill the fields from what the daemon says, once per session.
    private func load() {
        guard loaded != pane.id else { return }
        loaded = pane.id
        if let shift = pane.nightShift {
            pauseAt = Double(shift.pauseAt)
            resumePrompt = shift.resumePrompt
        } else {
            pauseAt = 90
            resumePrompt = ""
        }
        queueText = ""
    }

    private func apply() {
        store.send(.nightShift(
            pane: pane.id,
            plan: NightShiftPlan(
                pauseAt: UInt16(pauseAt),
                resumePrompt: resumePrompt,
                queue: prompts
            )
        ))
    }

    /// What the night shift is doing for this session now.
    private struct Running: View {
        let shift: NightShiftView

        var body: some View {
            HStack(spacing: 6) {
                Image(systemName: shift.paused ? "pause.circle" : "moon.stars")
                    .foregroundStyle(shift.paused ? AnyShapeStyle(.tint)
                        : AnyShapeStyle(.secondary))
                Text(label)
                    .font(.caption)
                Spacer(minLength: 0)
            }
            .padding(6)
            .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
        }

        private var label: String {
            if shift.paused {
                return "paused at \(shift.pauseAt)%, waiting for the window to reset"
            }
            if shift.queued > 0 {
                return "\(shift.sent) of \(shift.queued) sent"
            }
            return shift.sent > 0 ? "\(shift.sent) sent" : "waiting for the session to be idle"
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
            .padding(16)
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
                    .padding(16)
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
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
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
        .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 8))
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
