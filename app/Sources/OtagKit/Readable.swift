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
    /// zone, because that is the clock the user is looking at — but in the
    /// language otag is speaking, because `02:40` and `2:40 AM` and
    /// `klo 2.40` are the same moment written three ways and a user should
    /// see the one their language uses.
    public static func resetClock(
        _ resetsAt: Int64?,
        now: Date = Date(),
        locale: Locale? = nil
    ) -> String? {
        guard let resetsAt else { return nil }

        let wanted = locale ?? Self.locale
        let when = Date(timeIntervalSince1970: TimeInterval(resetsAt))
        let clock = when.formatted(
            Date.FormatStyle(date: .omitted, time: .shortened).locale(wanted)
        )
        if when.timeIntervalSince(now) < 24 * 60 * 60 {
            return clock
        }
        let day = when.formatted(
            Date.FormatStyle().day().month(.abbreviated).locale(wanted)
        )
        return "\(day) \(clock)"
    }

    /// The locale otag is formatting for.
    ///
    /// The chosen language's, so a reset time and a number are written the
    /// way that language writes them rather than the way the Mac does.
    ///
    /// Held here rather than read from `Strings` on demand, because these
    /// two functions are called from wherever a number is drawn and
    /// `Strings` belongs to the main actor. `Strings` pushes the language in
    /// instead; a lock keeps the read honest from any thread.
    public static var locale: Locale {
        lock.lock()
        defer { lock.unlock() }
        return chosen
    }

    /// Say which language numbers are written in from now on.
    ///
    /// Called by `Strings` when the language changes, and by nothing else.
    static func format(for locale: Locale) {
        lock.lock()
        defer { lock.unlock() }
        chosen = locale
    }

    /// `Mutex` would be the thing to use, and it needs macOS 15; this
    /// package supports 14.
    private static let lock = NSLock()
    private nonisolated(unsafe) static var chosen = Language.fallback.locale

    /// A token count: `840`, `12k`, `1.2M`.
    ///
    /// The decimal separator follows the language, so `1.2M` is `1,2M` in
    /// Turkish and Finnish.
    public static func tokens(_ count: UInt64, locale: Locale? = nil) -> String {
        let wanted = locale ?? Self.locale
        let thousand: UInt64 = 1_000
        let million: UInt64 = 1_000_000

        if count >= million {
            let millions = Double(count) / Double(million)
            return millions < 10
                ? millions.formatted(
                    .number.precision(.fractionLength(1)).locale(wanted)
                ) + "M"
                : "\(UInt64(millions.rounded()))M"
        }
        if count >= thousand {
            return "\(count / thousand)k"
        }
        return "\(count)"
    }
}
