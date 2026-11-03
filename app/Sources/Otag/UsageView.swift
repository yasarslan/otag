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
                if store.usage.isFresh {
                    ForEach(store.usage.rows) { row in
                        UsageRowView(row: row)
                    }
                } else {
                    // Nothing in the report describes the limits as they are
                    // now, and a number nobody has confirmed is worse than
                    // no number: it is the one thing a meter must not imply.
                    UsageHint(store: store)
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

/// Ask the daemon for the numbers again.
///
/// Small and quiet, beside the numbers it refreshes. Its tooltip is where
/// the provenance lives: when the last reading arrived and which session
/// otag heard it from.
struct UsageRefresh: View {
    let store: SessionStore

    var body: some View {
        Button {
            store.refreshUsage()
        } label: {
            Image(systemName: "arrow.clockwise")
                .font(.caption)
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
        .help(store.usage.provenance(panes: store.panes))
        .accessibilityLabel("Refresh usage")
    }
}

/// What the widget says instead of a number nobody has confirmed.
struct UsageHint: View {
    let store: SessionStore

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            VStack(alignment: .leading, spacing: 2) {
                Text(headline)
                    .font(.caption)
                Text("An agent reports the limits as it works.")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 4)
            UsageRefresh(store: store)
        }
    }

    /// One line on why there is no number, which is not always the same
    /// reason: a window that has reset is waiting for its first reading,
    /// and an old one was simply never confirmed.
    private var headline: String {
        if store.usage.rows.contains(where: \.reset) {
            return "Window reset — waiting for fresh data"
        }
        if let age = store.usage.age {
            return "No reading for \(Readable.duration(age))"
        }
        return "No reading yet"
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
                if let percent = row.usedPercent {
                    Text("\(percent)%")
                        .monospacedDigit()
                        .foregroundStyle(colour)
                } else {
                    // The last percentage otag was told belongs to a window
                    // that is over; showing it as this window's would be the
                    // one wrong thing to say.
                    Text("reset")
                        .foregroundStyle(.secondary)
                }
            }
            .font(.caption)

            Meter(fraction: row.fraction, colour: colour)
                .opacity(row.reset || row.isStale ? 0.4 : 1)

            if let detail {
                Text(detail)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
    }

    /// A window waiting for its first reading says so, and an old reading
    /// says how old it is: both change what the number above means. After
    /// that the projection, because it is the useful number, and the reset
    /// when nothing is climbing.
    private var detail: String? {
        if row.reset {
            return "waiting for fresh data"
        }
        if row.isStale {
            return Readable.ago(row.age)
        }
        if let full = row.fullIn {
            return "full in \(Readable.duration(full))"
        }
        if let resets = row.resetsIn, resets > 0 {
            return "resets \(Readable.duration(resets))"
        }
        return nil
    }

    /// A number otag cannot vouch for is never coloured: an old reading must
    /// not shout, and a window waiting for data has nothing to shout about.
    private var colour: Color {
        if row.reset || row.isStale { return .secondary }
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
                    section("Limits", accessory: { UsageRefresh(store: store) }) {
                        if !store.usage.isFresh {
                            UsageHint(store: store)
                        }
                        ForEach(store.usage.rows) { row in
                            UsageRowView(row: row)
                        }
                        Text(store.usage.provenance(panes: store.panes))
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                    section("Timeline") {
                        UsageChart(samples: store.history)
                    }
                }

                if let pane {
                    section("This session") {
                        ContextBar(pane: pane)
                        HandoffButtons(store: store, pane: pane)
                    }
                    section("Night shift") {
                        NightShiftPanel(store: store, pane: pane)
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
        section(title, accessory: { EmptyView() }, content: content)
    }

    /// A section with something small on the right of its title.
    private func section<Accessory: View, Content: View>(
        _ title: String,
        @ViewBuilder accessory: () -> Accessory,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
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
