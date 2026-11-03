import Foundation

/// Numbers written the way a person reads them at a glance.
///
/// The same rules as the terminal client's, so the two never disagree about
/// the same number: a reset is a time of day, and a token count is
/// shortened but never rounded to nothing.
public enum Readable {
    /// What a window with no reading yet shows instead of a percentage.
    public static let noReading = "–"

    /// When a window resets, as a person reads a clock: `02:40` tonight, and
    /// `10 Nov 06:00` for a week away.
    ///
    /// The same rule as the terminal client's: within the day the clock
    /// alone is how a person thinks of it, and further out the clock alone
    /// would be a riddle, so it carries its date. In the machine's own time
    /// zone, because that is the clock the user is looking at.
    public static func resetClock(_ resetsAt: Int64?, now: Date = Date()) -> String? {
        guard let resetsAt else { return nil }

        let when = Date(timeIntervalSince1970: TimeInterval(resetsAt))
        let clock = when.formatted(
            Date.FormatStyle(date: .omitted, time: .shortened)
        )
        if when.timeIntervalSince(now) < 24 * 60 * 60 {
            return clock
        }
        let day = when.formatted(
            Date.FormatStyle().day().month(.abbreviated)
        )
        return "\(day) \(clock)"
    }

    /// A token count: `840`, `12k`, `1.2M`.
    public static func tokens(_ count: UInt64) -> String {
        let thousand: UInt64 = 1_000
        let million: UInt64 = 1_000_000

        if count >= million {
            let millions = Double(count) / Double(million)
            return millions < 10
                ? String(format: "%.1fM", millions)
                : "\(UInt64(millions.rounded()))M"
        }
        if count >= thousand {
            return "\(count / thousand)k"
        }
        return "\(count)"
    }
}
