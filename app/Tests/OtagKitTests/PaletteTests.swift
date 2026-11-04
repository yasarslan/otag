import Foundation
import Testing

@testable import OtagKit

/// The colours every pane draws with.
///
/// One palette for every session, whatever is running in it: a pane is a
/// pane. These check the numbers against themselves, which is the part a
/// test can see — the rest is a screen to look at.
@Suite("The terminal palette")
struct PaletteTests {
    @Test("both appearances name all sixteen colours")
    func sixteenEach() {
        #expect(TerminalPalette.dark.count == TerminalPalette.count)
        #expect(TerminalPalette.light.count == TerminalPalette.count)
        #expect(TerminalPalette.colors(dark: true) == TerminalPalette.dark)
        #expect(TerminalPalette.colors(dark: false) == TerminalPalette.light)
    }

    @Test("no colour is the same as another in its palette")
    func everyColourIsItsOwn() {
        // Two identical entries would make one of them invisible: a program
        // drawing in colour 3 and colour 11 would produce the same thing.
        for (name, palette) in [("dark", TerminalPalette.dark), ("light", TerminalPalette.light)] {
            #expect(Set(palette).count == palette.count, "\(name) repeats a colour")
        }
    }

    @Test("the bright half really is brighter")
    func brightIsBrighter() {
        // Bold text uses the bright half. One that is not brighter than its
        // normal counterpart makes bold text vanish instead of standing out.
        for index in 0 ..< 8 {
            let normal = TerminalPalette.luminance(TerminalPalette.dark[index])
            let bright = TerminalPalette.luminance(TerminalPalette.dark[index + 8])
            #expect(bright > normal, "dark colour \(index) does not brighten")

            let plainLight = TerminalPalette.luminance(TerminalPalette.light[index])
            let brightLight = TerminalPalette.luminance(TerminalPalette.light[index + 8])
            #expect(brightLight > plainLight, "light colour \(index) does not brighten")
        }
    }

    @Test("a dark pane's colours are lighter than a light pane's")
    func eachPaletteSuitsItsBackground() {
        // The same red has to be readable on near-black and on white, which
        // one set of numbers cannot do. The six colours — red through cyan —
        // are the ones that carry meaning, so they are the ones that matter.
        for index in 1 ... 6 {
            let onDark = TerminalPalette.luminance(TerminalPalette.dark[index])
            let onLight = TerminalPalette.luminance(TerminalPalette.light[index])
            #expect(
                onDark > onLight,
                "colour \(index) is not lighter for a dark pane than for a light one"
            )
        }
    }

    @Test("a colour comes apart into the components a terminal wants")
    func componentsOfAColour() {
        let parts = TerminalPalette.components(0x12_34_56)
        #expect(parts.red == 0x12)
        #expect(parts.green == 0x34)
        #expect(parts.blue == 0x56)

        #expect(TerminalPalette.luminance(0xFFFFFF) == 1)
        #expect(TerminalPalette.luminance(0x000000) == 0)
    }
}
