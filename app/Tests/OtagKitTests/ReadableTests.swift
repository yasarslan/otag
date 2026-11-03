import Foundation
import Testing

@testable import OtagKit

/// The app and the terminal client show the same numbers, so they are
/// written by the same rules. These are the terminal client's own test cases.
@Suite("Numbers a person reads")
struct ReadableTests {
    @Test("a duration is read at a glance")
    func durations() {
        #expect(Readable.duration(30) == "30s")
        #expect(Readable.duration(90) == "1m")
        #expect(Readable.duration(2_700) == "45m")
        #expect(Readable.duration(4_800) == "1h20m")
        #expect(Readable.duration(7_200) == "2h")
        #expect(Readable.duration(-5) == "0s", "a window that has reset is not negative")
    }

    @Test("a reset days away is shown in days")
    func longDurations() {
        #expect(Readable.duration(579_600) == "6d 17h")
        #expect(Readable.duration(86_400) == "1d")
        #expect(Readable.duration(86_399) == "23h59m")
        #expect(Readable.duration(604_800) == "7d")
    }

    @Test("a token count is shortened but never rounded to nothing")
    func tokens() {
        #expect(Readable.tokens(0) == "0")
        #expect(Readable.tokens(840) == "840")
        #expect(Readable.tokens(12_300) == "12k")
        #expect(Readable.tokens(1_234_567) == "1.2M")
        #expect(Readable.tokens(42_000_000) == "42M")
    }
}
