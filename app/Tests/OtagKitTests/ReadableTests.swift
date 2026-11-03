import Foundation
import Testing

@testable import OtagKit

/// The app and the terminal client show the same numbers, so they are
/// written by the same rules. These are the terminal client's own test cases.
@Suite("Numbers a person reads")
struct ReadableTests {
    @Test("a token count is shortened but never rounded to nothing")
    func tokens() {
        #expect(Readable.tokens(0) == "0")
        #expect(Readable.tokens(840) == "840")
        #expect(Readable.tokens(12_300) == "12k")
        #expect(Readable.tokens(1_234_567) == "1.2M")
        #expect(Readable.tokens(42_000_000) == "42M")
    }
}
