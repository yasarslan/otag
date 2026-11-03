import Charts
import OtagKit
import SwiftUI

/// How much of each agent's limits is gone, at the bottom of the sidebar.
///
/// Calm, like the rest: a thin bar, a number, and the one useful line under
/// it — when the window would be full at the rate of the last twenty
/// minutes, or when it resets if nothing is climbing.
struct UsageWidget: View {
    let store: SessionStore

    var body: some View {
        if !store.usage.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(store.usage.rows) { row in
                    UsageRowView(row: row)
                }
                ForEach(store.usage.counted) { counted in
                    // A count, not a percentage: nobody knows what such an
                    // account's allowance is.
                    Text("\(counted.agent.label) ~\(Readable.tokens(counted.tokens))")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
    }
}

/// One window.
struct UsageRowView: View {
    let row: UsageRow

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 4) {
                Text(row.agent.label)
                    .lineLimit(1)
                Text(row.window.label)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 4)
                Text("\(row.usedPercent)%")
                    .monospacedDigit()
                    .foregroundStyle(colour)
            }
            .font(.caption)

            Meter(fraction: Double(row.usedTenths) / 1_000, colour: colour)

            if let detail {
                Text(detail)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
    }

    /// The projection comes first because it is the useful number; the reset
    /// is shown instead when nothing is climbing.
    private var detail: String? {
        if let full = row.fullIn {
            return "full in \(Readable.duration(full))"
        }
        if let resets = row.resetsIn, resets > 0 {
            return "resets \(Readable.duration(resets))"
        }
        return nil
    }

    private var colour: Color {
        switch row.level {
        case .calm: return .secondary
        case .amber: return .orange
        case .red: return .red
        }
    }
}

/// A thin bar. No animation, no gradient, nothing that fills up as you
/// watch: it is a number you glance at.
struct Meter: View {
    let fraction: Double
    let colour: Color

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule()
                    .fill(.quaternary)
                Capsule()
                    .fill(colour.opacity(0.7))
                    .frame(width: geometry.size.width * min(max(fraction, 0), 1))
            }
        }
        .frame(height: 3)
    }
}

/// The inspector: the forecast, the timeline, and what each session is
/// spending.
struct UsageInspector: View {
    let store: SessionStore
    /// The session the window is showing, whose context is worth a bar.
    let pane: PaneInfo?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                if store.usage.isEmpty {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("No numbers yet")
                            .font(.headline)
                        Text("The agents report their limits as they work. For Claude Code "
                            + "that needs otag's hooks: run `otag install-hooks`.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                } else {
                    section("Limits") {
                        ForEach(store.usage.rows) { row in
                            UsageRowView(row: row)
                        }
                    }
                    section("Timeline") {
                        UsageChart(samples: store.history)
                    }
                }

                if let pane {
                    section("This session") {
                        ContextBar(pane: pane)
                    }
                }

                section("What each session is spending") {
                    ForEach(spenders) { pane in
                        HStack(spacing: 6) {
                            Text(pane.title.isEmpty
                                ? (pane.agent?.label ?? "shell") : pane.title)
                                .lineLimit(1)
                            Spacer(minLength: 6)
                            Text(Readable.tokens(pane.tokens.input + pane.tokens.output
                                + pane.tokens.cacheRead + pane.tokens.cacheWritten))
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
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// The sessions that have spent something, most first.
    private var spenders: [PaneInfo] {
        store.panes
            .filter { $0.tokens.input + $0.tokens.output + $0.tokens.cacheRead > 0 }
            .sorted {
                $0.tokens.input + $0.tokens.output + $0.tokens.cacheRead
                    > $1.tokens.input + $1.tokens.output + $1.tokens.cacheRead
            }
    }

    private func section<Content: View>(
        _ title: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.subheadline.weight(.semibold))
            content()
        }
    }
}

/// How much of the limits was gone, over time.
struct UsageChart: View {
    let samples: [UsageSample]

    var body: some View {
        if samples.count < 2 {
            Text("A chart needs a few readings. otag keeps them across restarts, so this "
                + "fills in as the agents work.")
                .font(.caption)
                .foregroundStyle(.secondary)
        } else {
            Chart(samples) { sample in
                LineMark(
                    x: .value("When", sample.date),
                    y: .value("Used", sample.usedPercent)
                )
                .foregroundStyle(by: .value("Window", label(sample)))
                .interpolationMethod(.monotone)
            }
            .chartYScale(domain: 0 ... 100)
            .chartYAxis {
                AxisMarks(values: [0, 50, 100]) { value in
                    AxisGridLine()
                    AxisValueLabel {
                        if let percent = value.as(Int.self) {
                            Text("\(percent)%")
                        }
                    }
                }
            }
            .chartLegend(position: .bottom, spacing: 6)
            .frame(height: 140)
        }
    }

    private func label(_ sample: UsageSample) -> String {
        "\(sample.agent.label) \(sample.window.label)"
    }
}

/// How much of a session's context is in use.
///
/// Only a bar when otag knows how large the model's context is; otherwise
/// the count alone, because a share of an unknown total is a guess.
struct ContextBar: View {
    let pane: PaneInfo

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Text("Context")
                    .foregroundStyle(.secondary)
                Spacer(minLength: 6)
                Text(detail)
                    .monospacedDigit()
            }
            .font(.caption)

            if let fraction {
                Meter(fraction: fraction, colour: fraction > 0.8 ? .red
                    : (fraction > 0.6 ? .orange : .secondary))
            }
        }
    }

    private var fraction: Double? {
        guard let window = pane.contextWindow, window > 0 else { return nil }
        return Double(pane.tokens.context) / Double(window)
    }

    private var detail: String {
        let used = Readable.tokens(pane.tokens.context)
        guard let window = pane.contextWindow, window > 0 else { return used }
        let percent = Int((Double(pane.tokens.context) / Double(window) * 100).rounded())
        return "\(used) of \(Readable.tokens(window)) · \(percent)%"
    }
}
