import Foundation
import Testing

@testable import OtagKit

/// The three terminal themes.
///
/// A screenshot is something I cannot look at; a contrast ratio is something
/// a test can. These check each theme against itself: sixteen colours, none
/// repeated, a bright half that really is brighter, and enough contrast with
/// the theme's own paper that an agent's diff blocks and highlighted prompts
/// stay readable.
@Suite("Terminal themes")
struct TerminalThemeTests {
    @Test("there are three, and they are told apart by their identifiers")
    func three() {
        #expect(TerminalTheme.all.count == 3)
        #expect(Set(TerminalTheme.all.map(\.id)).count == 3)
        #expect(Set(TerminalTheme.all.map(\.name)).count == 3)
        for theme in TerminalTheme.all {
            #expect(TerminalTheme.named(theme.id) == theme)
        }
        #expect(TerminalTheme.named("no-such-theme") == nil)
    }

    @Test("each theme names all sixteen colours, and none of them twice")
    func sixteenEach() {
        for theme in TerminalTheme.all {
            #expect(
                theme.ansi.count == TerminalTheme.ansiCount,
                "\(theme.name) does not have sixteen colours"
            )
            // Two identical entries would make one of them invisible: a
            // program drawing in colour 3 and colour 11 would produce the
            // same thing.
            #expect(Set(theme.ansi).count == theme.ansi.count, "\(theme.name) repeats a colour")
        }
    }

    @Test("the bright half really is brighter")
    func brightIsBrighter() {
        // Bold text uses the bright half. One that is not brighter than its
        // normal counterpart makes bold text vanish instead of stand out.
        for theme in TerminalTheme.all {
            for index in 0 ..< 8 {
                let normal = TerminalPalette.luminance(theme.ansi[index])
                let bright = TerminalPalette.luminance(theme.ansi[index + 8])
                #expect(bright > normal, "\(theme.name) colour \(index) does not brighten")
            }
        }
    }

    @Test("text is comfortably readable on every theme's own paper")
    func textContrast() {
        for theme in TerminalTheme.all {
            let ratio = TerminalPalette.contrast(theme.foreground, theme.background)
            #expect(
                ratio >= TerminalPalette.textContrast,
                "\(theme.name) text is \(ratio) against its background"
            )
        }
    }

    @Test("every colour that carries meaning is readable on every theme")
    func colourContrast() {
        // This is the one that keeps an agent's interface usable: a diff's
        // green, a prompt's highlighted border, a warning's yellow.
        for theme in TerminalTheme.all {
            for index in TerminalPalette.meaningfulColours {
                let ratio = TerminalPalette.contrast(theme.ansi[index], theme.background)
                #expect(
                    ratio >= TerminalPalette.colourContrast,
                    "\(theme.name) colour \(index) is \(ratio) against its background"
                )
            }
        }
    }

    @Test("the cursor and the selection can be seen")
    func cursorAndSelection() {
        for theme in TerminalTheme.all {
            #expect(
                TerminalPalette.contrast(theme.cursor, theme.background) >= 3,
                "\(theme.name) has a cursor nobody can find"
            )
            // Selected text keeps the theme's own ink on top of it.
            #expect(
                TerminalPalette.contrast(theme.foreground, theme.selection) >= 4.5,
                "\(theme.name) loses its text when it is selected"
            )
        }
    }

    @Test("a dark theme's paper is dark and a light theme's is light")
    func paperMatchesItsName() {
        #expect(!TerminalTheme.glassLight.isDark)
        #expect(TerminalTheme.glassDark.isDark)
        #expect(TerminalTheme.midnight.isDark)
        for theme in TerminalTheme.all {
            let light = TerminalPalette.relativeLuminance(theme.background)
            #expect(theme.isDark == (light < 0.3), "\(theme.name) is not the look it claims")
        }
    }

    @Test("Midnight really is navy, and brighter than Glass Dark")
    func midnightIsItsOwnThing() {
        let navy = TerminalPalette.components(TerminalTheme.midnight.background)
        #expect(navy.blue > navy.red, "Midnight's paper is not blue")
        #expect(navy.blue > navy.green)

        // Its colours sit on a darker, bluer paper, so they have to be
        // pushed further than Glass Dark's to clear the same bar.
        for index in 1 ... 6 {
            #expect(
                TerminalPalette.luminance(TerminalTheme.midnight.ansi[index])
                    > TerminalPalette.luminance(TerminalTheme.glassDark.ansi[index]),
                "Midnight colour \(index) is not brighter than Glass Dark's"
            )
        }
    }

    @Test("a light theme's colours are darker than a dark theme's")
    func eachPaletteSuitsItsPaper() {
        // The same red has to be readable on near-white and on near-black,
        // which one set of numbers cannot do.
        for index in 1 ... 6 {
            #expect(
                TerminalPalette.luminance(TerminalTheme.glassLight.ansi[index])
                    < TerminalPalette.luminance(TerminalTheme.glassDark.ansi[index]),
                "colour \(index) is not darker for a light pane than for a dark one"
            )
        }
    }

    @Test("following the system means Glass Light or Glass Dark")
    func systemChoice() {
        #expect(TerminalThemeChoice.system.resolved(dark: false) == .glassLight)
        #expect(TerminalThemeChoice.system.resolved(dark: true) == .glassDark)
    }

    @Test("a chosen theme is the same in either appearance")
    func aChosenThemeDoesNotFollowTheSystem() {
        for choice in TerminalThemeChoice.allCases where choice != .system {
            #expect(
                choice.resolved(dark: true) == choice.resolved(dark: false),
                "\(choice.label) changes with the appearance"
            )
        }
        #expect(TerminalThemeChoice.midnight.resolved(dark: false) == .midnight)
    }

    @Test("a preference this build cannot read means following the system")
    func anUnknownPreference() {
        #expect(TerminalThemeChoice.stored(nil) == .system)
        #expect(TerminalThemeChoice.stored("") == .system)
        #expect(TerminalThemeChoice.stored("solarized") == .system)
        #expect(TerminalThemeChoice.stored("midnight") == .midnight)
        #expect(TerminalThemeChoice.stored("glass-light") == .glassLight)
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

    @Test("the contrast ratio is the one the standard defines")
    func theContrastFormula() {
        // Black on white is 21:1, and anything against itself is 1:1.
        #expect(abs(TerminalPalette.contrast(0x000000, 0xFFFFFF) - 21) < 0.01)
        #expect(TerminalPalette.contrast(0x808080, 0x808080) == 1)
        // It does not matter which way round the two are given.
        #expect(
            TerminalPalette.contrast(0x123456, 0xFEDCBA)
                == TerminalPalette.contrast(0xFEDCBA, 0x123456)
        )
    }
}
