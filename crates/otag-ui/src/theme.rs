//! Colours otag draws its own furniture in.
//!
//! The rules behind the palettes: nothing pure white or pure black, no
//! saturated colour except for a real error, and attention drawn with a thin
//! border rather than with a bright fill or anything that blinks. What a
//! pane's program draws is never recoloured — only otag's own sidebar, tab
//! bar and borders use these.
//!
//! Neither the background nor the plain text colour is otag's to choose:
//! both are the terminal's own. Painting a background of its own would put
//! bands of the wrong colour behind every agent UI that leaves its own
//! background alone, and would fight the user's terminal theme for no gain.

use ratatui::style::{Color, Modifier, Style};

/// One palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Behind otag's own furniture: the terminal's own background.
    pub background: Color,
    /// Text that matters: the terminal's own foreground.
    pub text: Color,
    /// Text that is only there when looked for.
    pub muted: Color,
    /// Border of a pane that is not focused.
    pub border: Color,
    /// Border of the focused pane.
    pub border_focus: Color,
    /// The one colour used to pick something out.
    pub accent: Color,
    /// Something wants the user.
    pub attention: Color,
    /// Something went wrong.
    pub error: Color,
}

impl Theme {
    /// The default: a low-contrast dark palette.
    pub const CALM_DARK: Self = Self {
        background: Color::Reset,
        text: Color::Reset,
        muted: Color::Rgb(0x6c, 0x71, 0x7a),
        border: Color::Rgb(0x3a, 0x3f, 0x47),
        border_focus: Color::Rgb(0x7c, 0x9c, 0xbf),
        accent: Color::Rgb(0x8f, 0xb8, 0xb4),
        attention: Color::Rgb(0xd1, 0xa0, 0x6c),
        error: Color::Rgb(0xc4, 0x7e, 0x7e),
    };

    /// A low-contrast light palette.
    pub const CALM_LIGHT: Self = Self {
        background: Color::Reset,
        text: Color::Reset,
        muted: Color::Rgb(0x87, 0x8c, 0x94),
        border: Color::Rgb(0xd2, 0xcf, 0xc8),
        border_focus: Color::Rgb(0x4f, 0x74, 0x96),
        accent: Color::Rgb(0x44, 0x7a, 0x74),
        attention: Color::Rgb(0x9a, 0x6a, 0x2c),
        error: Color::Rgb(0x9c, 0x4b, 0x4b),
    };

    /// Plain text on otag's own background.
    #[must_use]
    pub const fn plain(self) -> Style {
        Style::new().fg(self.text).bg(self.background)
    }

    /// Text that should not draw the eye.
    #[must_use]
    pub const fn dim(self) -> Style {
        Style::new().fg(self.muted).bg(self.background)
    }

    /// Text picked out from the rest.
    #[must_use]
    pub fn picked_out(self) -> Style {
        Style::new()
            .fg(self.accent)
            .bg(self.background)
            .add_modifier(Modifier::BOLD)
    }

    /// A pane's border, thinner-looking when the pane is not focused.
    #[must_use]
    pub const fn pane_border(self, focused: bool) -> Style {
        Style::new()
            .fg(if focused {
                self.border_focus
            } else {
                self.border
            })
            .bg(self.background)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::CALM_DARK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_theme_is_the_calm_dark_one() {
        assert_eq!(Theme::default(), Theme::CALM_DARK);
    }

    #[test]
    fn no_palette_paints_a_background_or_recolours_plain_text() {
        for theme in [Theme::CALM_DARK, Theme::CALM_LIGHT] {
            assert_eq!(theme.background, Color::Reset);
            assert_eq!(theme.text, Color::Reset);
            assert_eq!(
                theme.plain(),
                Style::new().fg(Color::Reset).bg(Color::Reset)
            );
        }
    }

    #[test]
    fn no_palette_uses_pure_black_or_pure_white() {
        for theme in [Theme::CALM_DARK, Theme::CALM_LIGHT] {
            for colour in [
                theme.background,
                theme.text,
                theme.muted,
                theme.border,
                theme.border_focus,
                theme.accent,
                theme.attention,
                theme.error,
            ] {
                assert_ne!(colour, Color::Rgb(0, 0, 0));
                assert_ne!(colour, Color::Rgb(0xff, 0xff, 0xff));
            }
        }
    }

    #[test]
    fn the_focused_border_differs_from_the_others() {
        let theme = Theme::default();
        assert_ne!(theme.pane_border(true), theme.pane_border(false));
    }
}
