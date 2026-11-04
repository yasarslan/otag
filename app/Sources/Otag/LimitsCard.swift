import Charts
import OtagKit
import SwiftUI

/// How much of each agent's limits is gone, at the foot of the sidebar.
///
/// One compact card: a line per window, saying what the agent last reported
/// and when that window resets, plus whether otag is talking to the daemon at
/// all. The whole card is a button; everything that used to be an inspector
/// panel on the other side of the window is behind it.
struct LimitsCard: View {
    let store: SessionStore
    /// The session with the keyboard, whose context is worth a bar in the
    /// popover.
    let focused: PaneInfo?

    @State private var open = false

    var body: some View {
        Button {
            open = true
        } label: {
            VStack(alignment: .leading, spacing: 5) {
                if store.usage.rows.isEmpty, store.usage.counted.isEmpty {
                    Text("Limits")
                        .font(.caption.weight(.medium))
                    Text("Nothing reported yet.")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(store.usage.rows) { row in
                        CompactUsageRow(row: row)
                    }
                    ForEach(store.usage.counted) { counted in
                        // A count, not a percentage: nobody knows what such
                        // an account's allowance is.
                        Text("\(counted.agent.label)  ~\(Readable.tokens(counted.tokens))")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                }

                Connection(store: store)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10)
            .cardBackground()
            .contentShape(RoundedRectangle(cornerRadius: Chrome.cardRadius))
        }
        .buttonStyle(.plain)
        .padding(10)
        .help("Limits, the timeline, and what each session is spending")
        .popover(isPresented: $open, arrowEdge: .trailing) {
            LimitsPopover(store: store, focused: focused)
        }
    }
}

/// One window, in as little room as it can be said in.
private struct CompactUsageRow: View {
    let row: UsageRow

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 4) {
                Text(row.agent.label)
                    .lineLimit(1)
                Text(row.window.label)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 4)
                Text(used)
                    .monospacedDigit()
                    .foregroundStyle(colour)
                if let resets = Readable.resetClock(row.resetsAt) {
                    Text(resets)
                        .foregroundStyle(.tertiary)
                        .monospacedDigit()
                }
            }
            .font(.caption2)

            Meter(fraction: row.fraction, colour: colour)
        }
    }

    /// The percentage, or a dash when otag has never had a reading of this
    /// window. A dash says what no percentage can say.
    private var used: String {
        guard let percent = row.usedPercent else { return Readable.noReading }
        return "\(percent)%"
    }

    private var colour: Color {
        guard row.usedTenths != nil else { return .secondary }
        switch row.level {
        case .calm: return .secondary
        case .amber: return .orange
        case .red: return .red
        }
    }
}

/// Whether the app is talking to the daemon.
///
/// One dot and a few words, at the bottom of the card: the one thing about
/// otag a user cannot otherwise see.
private struct Connection: View {
    let store: SessionStore

    var body: some View {
        HStack(spacing: 5) {
            Circle()
                .fill(colour)
                .frame(width: 6, height: 6)
            Text(label)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 0)
        }
        .padding(.top, 1)
        .help(store.socket.path)
    }

    private var colour: Color {
        switch store.state {
        case .connected: return .green
        case .connecting: return .orange
        case .failed: return .red
        }
    }

    private var label: String {
        switch store.state {
        case .connected:
            if let version = store.daemonVersion { return "daemon \(version)" }
            return "connected"
        case .connecting:
            return "connecting…"
        case let .failed(message):
            return message
        }
    }
}

/// Everything about the limits, in the one place that is about reading them.
///
/// This used to be an inspector panel on the other side of the window, which
/// meant two places to look for one subject. It is the same content: the
/// limits, the timeline, what each session is spending, and the context of
/// whatever has the keyboard.
struct LimitsPopover: View {
    let store: SessionStore
    let focused: PaneInfo?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                section("Limits", accessory: { UsageRefresh(store: store) }) {
                    ForEach(store.usage.rows) { row in
                        UsageRowView(row: row)
                    }
                    ForEach(store.usage.counted) { counted in
                        Text("\(counted.agent.label) ~\(Readable.tokens(counted.tokens))")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    if store.usage.isEmpty {
                        Text("No agent has reported a reading yet. otag never asks an agent "
                            + "for your limits itself — that would spend tokens.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }

                if !store.usage.isEmpty {
                    section("Timeline") {
                        UsageChart(samples: store.history)
                    }
                }

                if let focused {
                    section("This session") {
                        ContextBar(pane: focused)
                    }
                }

                section("What each session is spending") {
                    ForEach(spenders) { pane in
                        HStack(spacing: 6) {
                            AgentMark(agent: pane.agent, size: 7)
                            Text(pane.workspace.isEmpty
                                ? pane.agent.name : pane.workspace)
                                .lineLimit(1)
                            Spacer(minLength: 6)
                            Text(Readable.tokens(spent(pane)))
                                .monospacedDigit()
                                .foregroundStyle(.secondary)
                        }
                        .font(.caption)
                    }
                    if spenders.isEmpty {
                        Text("Nothing counted yet.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .padding(Chrome.gutter)
            .frame(width: 320, alignment: .leading)
        }
        .frame(maxHeight: 520)
    }

    private func spent(_ pane: PaneInfo) -> UInt64 {
        pane.tokens.input + pane.tokens.output + pane.tokens.cacheRead
            + pane.tokens.cacheWritten
    }

    /// The sessions that have spent something, most first.
    private var spenders: [PaneInfo] {
        store.panes
            .filter { spent($0) > 0 }
            .sorted { spent($0) > spent($1) }
    }

    private func section<Content: View>(
        _ title: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        section(title, accessory: { EmptyView() }, content: content)
    }

    /// A section with something small on the right of its title.
    private func section<Accessory: View, Content: View>(
        _ title: String,
        @ViewBuilder accessory: () -> Accessory,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: Chrome.gap) {
            HStack(spacing: 6) {
                Text(title)
                    .font(.subheadline.weight(.semibold))
                Spacer(minLength: 4)
                accessory()
            }
            content()
        }
    }
}
