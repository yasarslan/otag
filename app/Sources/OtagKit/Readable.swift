import Foundation

/// Numbers written the way a person reads them at a glance.
///
/// The same rules as the terminal client's, so the two never disagree about
/// the same number: a duration past a day drops its minutes, and a token
/// count is shortened but never rounded to nothing.
public enum Readable {
    /// A duration: `6d 17h`, `2h`, `1h20m`, `45m`, `30s`.
    ///
    /// Past a day the minutes stop mattering; `161h` is not a number anyone
    /// reads.
    public static func duration(_ seconds: Int64) -> String {
        let seconds = max(seconds, 0)
        let minutes = seconds / 60
        let hours = minutes / 60

        if hours >= 24 {
            let days = hours / 24
            let rest = hours % 24
            return rest == 0 ? "\(days)d" : "\(days)d \(rest)h"
        }
        if hours > 0 {
            let rest = minutes % 60
            return rest == 0 ? "\(hours)h" : "\(hours)h\(String(format: "%02d", rest))m"
        }
        if minutes > 0 {
            return "\(minutes)m"
        }
        return "\(seconds)s"
    }

    /// How long ago a reading was taken: `updated 3h ago`.
    public static func ago(_ seconds: Int64) -> String {
        "updated \(duration(seconds)) ago"
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
