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
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 4) {
                Text("Limits")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 4)
                UsageRefresh(store: store)
            }

            if store.usage.hasNumbers {
                // Each window says what it knows: a current number, a number
                // with its age on it, or that it is waiting for the first
                // reading of a window that has reset. One window being over
                // is no reason to hide another that is not.
                ForEach(store.usage.rows) { row in
                    UsageRowView(row: row)
                }
            } else {
                UsageHint(report: store.usage)
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

/// Ask the daemon for the numbers again.
///
/// Always there, beside the numbers it refreshes, because the one moment a
/// person wants it is the moment the numbers look wrong. Its tooltip carries
/// the provenance: when the last reading arrived and which session otag
/// heard it from.
struct UsageRefresh: View {
    let store: SessionStore

    /// Set while the icon turns, and again when the answer has landed, so
    /// pressing the button is visibly something rather than nothing.
    @State private var turn = 0.0
    @State private var justUpdated = false

    var body: some View {
        HStack(spacing: 4) {
            if justUpdated {
                Text("Updated")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .transition(.opacity)
            }
            Button {
                store.refreshUsage()
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .rotationEffect(.degrees(turn))
                    // A hit area a person can actually hit: the glyph itself
                    // is ten points across, and this is the whole square.
                    .frame(width: 20, height: 20)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.borderless)
            .help(store.usage.provenance(panes: store.panes))
            .accessibilityLabel("Refresh usage")
        }
        .onChange(of: store.refreshingUsage) { _, refreshing in
            // One turn of the icon per re-read, started when the request
            // goes out and finished when the answer arrives. Nothing spins
            // forever: the store stops waiting on its own.
            withAnimation(refreshing ? .linear(duration: 0.9).repeatForever(autoreverses: false)
                : .easeOut(duration: 0.2)) {
                turn = refreshing ? turn + 360 : turn.rounded()
            }
        }
        .onChange(of: store.usageUpdatedAt) {
            withAnimation { justUpdated = true }
            Task {
                try? await Task.sleep(for: .seconds(2))
                withAnimation { justUpdated = false }
            }
        }
    }
}

/// What the widget says when no window has a number to show.
struct UsageHint: View {
    let report: UsageReport

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(headline)
                .font(.caption)
            Text(UsageReport.whereNumbersComeFrom)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// Why there is nothing to show, which is not always the same reason: a
    /// window that has reset is waiting for the first reading of the new
    /// one, and an account otag has never heard about has none at all.
    private var headline: String {
        report.rows.isEmpty ? "No limits yet" : "Windows have reset"
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
                section("Limits", accessory: { UsageRefresh(store: store) }) {
                    if store.usage.hasNumbers {
                        ForEach(store.usage.rows) { row in
                            UsageRowView(row: row)
                        }
                        Text(store.usage.provenance(panes: store.panes))
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    } else {
                        // Nothing to draw, so this is the place to say where
                        // numbers come from at all. For Claude Code that is
                        // a reply in any session, which needs otag's hooks.
                        Text(store.usage.rows.isEmpty ? "No numbers yet" : "Windows have reset")
                            .font(.headline)
                        Text(UsageReport.whereNumbersComeFrom
                            + " If none ever do, otag's hooks are not installed: "
                            + "run `otag install-hooks`.")
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
