//! Turning key presses into the bytes a pane's program expects.
//!
//! What a terminal sends for a key is not fixed: it depends on modes the
//! program itself turns on. An arrow key is `ESC [ A` normally and `ESC O A`
//! once a full-screen program asks for application cursor keys, and pasted
//! text is wrapped in markers only if the program asked to tell a paste from
//! typing. The daemon sends the current modes with every frame, and this
//! module applies them, so programs receive what they are waiting for.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use otag_core::frame::InputModes;

/// Start of a paste, when the program asked to be told about pastes.
const PASTE_START: &[u8] = b"\x1b[200~";

/// End of a paste.
const PASTE_END: &[u8] = b"\x1b[201~";

/// The bytes a pane's program should receive for `event`, if any.
///
/// Returns `None` for anything that is not text or a command: a key release,
/// a modifier pressed on its own, or a key this build does not encode.
#[must_use]
pub fn encode(event: KeyEvent, modes: InputModes) -> Option<Vec<u8>> {
    // A terminal reports presses and repeats; a release is not input.
    if matches!(event.kind, KeyEventKind::Release) {
        return None;
    }

    let modifiers = event.modifiers;
    let bytes = match event.code {
        KeyCode::Char(character) => character_bytes(character, modifiers, event.state, modes)?,
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => {
            // A terminal's backspace is delete; with control it is the older
            // backspace, which is what programs expect for "delete word".
            if modifiers.contains(KeyModifiers::CONTROL) {
                vec![0x08]
            } else {
                vec![0x7f]
            }
        }
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => cursor_key(b'A', modifiers, modes),
        KeyCode::Down => cursor_key(b'B', modifiers, modes),
        KeyCode::Right => cursor_key(b'C', modifiers, modes),
        KeyCode::Left => cursor_key(b'D', modifiers, modes),
        KeyCode::Home => cursor_key(b'H', modifiers, modes),
        KeyCode::End => cursor_key(b'F', modifiers, modes),
        KeyCode::Insert => tilde_key(2, modifiers),
        KeyCode::Delete => tilde_key(3, modifiers),
        KeyCode::PageUp => tilde_key(5, modifiers),
        KeyCode::PageDown => tilde_key(6, modifiers),
        KeyCode::F(number) => function_key(number, modifiers)?,
        // Modifiers on their own, media keys and the rest are not input a
        // pane can use.
        _ => return None,
    };

    Some(with_alt(bytes, modifiers))
}

/// The bytes for pasted text.
///
/// A program that turned on paste markers is told where the paste begins and
/// ends, so that it can take the text literally instead of acting on it. The
/// end marker is removed from the text itself: pasted content must not be
/// able to end its own paste and have the rest treated as typing.
#[must_use]
pub fn encode_paste(text: &str, modes: InputModes) -> Vec<u8> {
    // A terminal sends a carriage return for a newline, including in a paste.
    let text = text.replace('\n', "\r");

    if !modes.bracketed_paste {
        return text.into_bytes();
    }

    let marker = String::from_utf8_lossy(PASTE_END).into_owned();
    let text = text.replace(&marker, "");

    let mut bytes = Vec::with_capacity(text.len() + PASTE_START.len() + PASTE_END.len());
    bytes.extend_from_slice(PASTE_START);
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(PASTE_END);
    bytes
}

/// Encode a printable key, a control combination, or a keypad key.
fn character_bytes(
    character: char,
    modifiers: KeyModifiers,
    state: KeyEventState,
    modes: InputModes,
) -> Option<Vec<u8>> {
    if modifiers.contains(KeyModifiers::CONTROL) {
        return control_byte(character).map(|byte| vec![byte]);
    }

    // A program that turned on the application keypad expects its own
    // sequences from the numeric keypad, which is how it tells the keypad
    // from the number row.
    if modes.application_keypad
        && state.contains(KeyEventState::KEYPAD)
        && let Some(byte) = keypad_byte(character)
    {
        return Some(vec![0x1b, b'O', byte]);
    }

    let mut buffer = [0_u8; 4];
    Some(character.encode_utf8(&mut buffer).as_bytes().to_vec())
}

/// The control character for a key held with control, as a terminal sends it.
fn control_byte(character: char) -> Option<u8> {
    let byte = match character.to_ascii_lowercase() {
        // Control turns a letter into the low byte of its position in the
        // alphabet: control-a is 1, control-z is 26.
        letter @ 'a'..='z' => (letter as u8) - b'a' + 1,
        ' ' | '2' | '@' => 0x00,
        '3' | '[' => 0x1b,
        '4' | '\\' => 0x1c,
        '5' | ']' => 0x1d,
        '6' | '^' => 0x1e,
        '7' | '-' | '_' => 0x1f,
        '8' | '?' => 0x7f,
        // Control with any other key is not something a terminal sends.
        _ => return None,
    };
    Some(byte)
}

/// The keypad key a character came from, in the order terminals use.
const fn keypad_byte(character: char) -> Option<u8> {
    let byte = match character {
        '0' => b'p',
        '1' => b'q',
        '2' => b'r',
        '3' => b's',
        '4' => b't',
        '5' => b'u',
        '6' => b'v',
        '7' => b'w',
        '8' => b'x',
        '9' => b'y',
        '-' => b'm',
        ',' => b'l',
        '.' => b'n',
        '+' => b'k',
        '*' => b'j',
        '/' => b'o',
        '=' => b'X',
        _ => return None,
    };
    Some(byte)
}

/// Encode a cursor or home key, which changes shape with the pane's modes.
fn cursor_key(final_byte: u8, modifiers: KeyModifiers, modes: InputModes) -> Vec<u8> {
    match modifier_parameter(modifiers) {
        // With a modifier, the key is always reported in the long form that
        // has room to name it.
        Some(parameter) => format!("\x1b[1;{parameter}{}", char::from(final_byte)).into_bytes(),
        None if modes.application_cursor => vec![0x1b, b'O', final_byte],
        None => vec![0x1b, b'[', final_byte],
    }
}

/// Encode one of the keys a terminal reports as a number.
fn tilde_key(number: u16, modifiers: KeyModifiers) -> Vec<u8> {
    match modifier_parameter(modifiers) {
        Some(parameter) => format!("\x1b[{number};{parameter}~").into_bytes(),
        None => format!("\x1b[{number}~").into_bytes(),
    }
}

/// Encode a function key. The first four have a form of their own, inherited
/// from the terminals these sequences come from.
fn function_key(number: u8, modifiers: KeyModifiers) -> Option<Vec<u8>> {
    let parameter = modifier_parameter(modifiers);

    if (1..=4).contains(&number) {
        let final_byte = b'P' + (number - 1);
        return Some(match parameter {
            Some(parameter) => format!("\x1b[1;{parameter}{}", char::from(final_byte)).into_bytes(),
            None => vec![0x1b, b'O', final_byte],
        });
    }

    // The numbers are not contiguous: they were assigned as keyboards grew,
    // leaving gaps where codes were already taken.
    let code = match number {
        5 => 15,
        6..=10 => u16::from(number) + 11,
        11..=14 => u16::from(number) + 12,
        15 | 16 => u16::from(number) + 13,
        17..=20 => u16::from(number) + 14,
        _ => return None,
    };
    Some(tilde_key(code, modifiers))
}

/// The number a terminal uses to report which modifiers were held.
///
/// Shift, alt and control are one, two and four, added to a base of one.
/// Alt is left out here: it is sent as a leading escape instead, which is
/// what terminals do by default and what programs recognise.
fn modifier_parameter(modifiers: KeyModifiers) -> Option<u8> {
    let mut held = 0;
    if modifiers.contains(KeyModifiers::SHIFT) {
        held |= 1;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        held |= 4;
    }
    (held != 0).then_some(held + 1)
}

/// Put the escape in front that marks a key held with alt.
fn with_alt(bytes: Vec<u8>, modifiers: KeyModifiers) -> Vec<u8> {
    if !modifiers.contains(KeyModifiers::ALT) {
        return bytes;
    }
    let mut prefixed = Vec::with_capacity(bytes.len() + 1);
    prefixed.push(0x1b);
    prefixed.extend_from_slice(&bytes);
    prefixed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Modes a program sets when it wants plain sequences.
    fn normal() -> InputModes {
        InputModes::default()
    }

    /// Modes a full-screen program such as an editor sets.
    fn application() -> InputModes {
        InputModes {
            application_cursor: true,
            application_keypad: true,
            ..InputModes::default()
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn encoded(event: KeyEvent, modes: InputModes) -> Vec<u8> {
        encode(event, modes).expect("the key produces input")
    }

    #[test]
    fn letters_and_digits_are_sent_as_they_are() {
        assert_eq!(encoded(key(KeyCode::Char('a')), normal()), b"a");
        assert_eq!(encoded(key(KeyCode::Char('Z')), normal()), b"Z");
        assert_eq!(encoded(key(KeyCode::Char('7')), normal()), b"7");
    }

    #[test]
    fn shift_is_already_in_the_character() {
        let event = with(KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert_eq!(encoded(event, normal()), b"A");
    }

    #[test]
    fn text_outside_ascii_is_sent_as_utf8() {
        assert_eq!(encoded(key(KeyCode::Char('ş')), normal()), "ş".as_bytes());
        assert_eq!(encoded(key(KeyCode::Char('→')), normal()), "→".as_bytes());
    }

    #[test]
    fn the_keys_that_end_a_line_or_move_by_a_tab() {
        assert_eq!(encoded(key(KeyCode::Enter), normal()), b"\r");
        assert_eq!(encoded(key(KeyCode::Tab), normal()), b"\t");
        assert_eq!(encoded(key(KeyCode::BackTab), normal()), b"\x1b[Z");
        assert_eq!(encoded(key(KeyCode::Esc), normal()), b"\x1b");
    }

    #[test]
    fn backspace_sends_delete_and_control_backspace_the_older_code() {
        assert_eq!(encoded(key(KeyCode::Backspace), normal()), [0x7f]);
        assert_eq!(
            encoded(with(KeyCode::Backspace, KeyModifiers::CONTROL), normal()),
            [0x08]
        );
    }

    #[test]
    fn control_combinations_become_control_characters() {
        // The ones that matter most: interrupt, end of file, and the key
        // otag itself uses as a prefix.
        assert_eq!(
            encoded(with(KeyCode::Char('c'), KeyModifiers::CONTROL), normal()),
            [0x03]
        );
        assert_eq!(
            encoded(with(KeyCode::Char('d'), KeyModifiers::CONTROL), normal()),
            [0x04]
        );
        assert_eq!(
            encoded(with(KeyCode::Char('b'), KeyModifiers::CONTROL), normal()),
            [0x02]
        );
        assert_eq!(
            encoded(with(KeyCode::Char('a'), KeyModifiers::CONTROL), normal()),
            [0x01]
        );
        assert_eq!(
            encoded(with(KeyCode::Char('z'), KeyModifiers::CONTROL), normal()),
            [0x1a]
        );
    }

    #[test]
    fn control_ignores_the_case_of_the_letter() {
        assert_eq!(
            encoded(with(KeyCode::Char('C'), KeyModifiers::CONTROL), normal()),
            [0x03]
        );
    }

    #[test]
    fn control_with_punctuation_follows_the_terminal_table() {
        for (character, byte) in [
            (' ', 0x00),
            ('@', 0x00),
            ('[', 0x1b),
            ('\\', 0x1c),
            (']', 0x1d),
            ('^', 0x1e),
            ('_', 0x1f),
            ('?', 0x7f),
        ] {
            assert_eq!(
                encoded(
                    with(KeyCode::Char(character), KeyModifiers::CONTROL),
                    normal()
                ),
                [byte],
                "control-{character}"
            );
        }
    }

    #[test]
    fn control_with_a_key_that_has_no_code_sends_nothing() {
        assert_eq!(
            encode(with(KeyCode::Char('ş'), KeyModifiers::CONTROL), normal()),
            None
        );
    }

    #[test]
    fn arrows_are_plain_sequences_in_normal_mode() {
        assert_eq!(encoded(key(KeyCode::Up), normal()), b"\x1b[A");
        assert_eq!(encoded(key(KeyCode::Down), normal()), b"\x1b[B");
        assert_eq!(encoded(key(KeyCode::Right), normal()), b"\x1b[C");
        assert_eq!(encoded(key(KeyCode::Left), normal()), b"\x1b[D");
    }

    #[test]
    fn arrows_change_shape_once_a_program_asks_for_application_cursor_keys() {
        // This is what an editor turns on, and why the mode has to travel
        // with every frame: the same key press has to be encoded differently.
        assert_eq!(encoded(key(KeyCode::Up), application()), b"\x1bOA");
        assert_eq!(encoded(key(KeyCode::Down), application()), b"\x1bOB");
        assert_eq!(encoded(key(KeyCode::Right), application()), b"\x1bOC");
        assert_eq!(encoded(key(KeyCode::Left), application()), b"\x1bOD");
    }

    #[test]
    fn home_and_end_follow_the_same_mode() {
        assert_eq!(encoded(key(KeyCode::Home), normal()), b"\x1b[H");
        assert_eq!(encoded(key(KeyCode::End), normal()), b"\x1b[F");
        assert_eq!(encoded(key(KeyCode::Home), application()), b"\x1bOH");
        assert_eq!(encoded(key(KeyCode::End), application()), b"\x1bOF");
    }

    #[test]
    fn a_modifier_puts_a_cursor_key_into_its_long_form_whatever_the_mode() {
        // With a modifier there is only one form, so application cursor mode
        // makes no difference.
        let shifted = with(KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(encoded(shifted, normal()), b"\x1b[1;2A");
        assert_eq!(encoded(shifted, application()), b"\x1b[1;2A");

        assert_eq!(
            encoded(with(KeyCode::Right, KeyModifiers::CONTROL), normal()),
            b"\x1b[1;5C"
        );
        assert_eq!(
            encoded(
                with(KeyCode::Left, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
                normal()
            ),
            b"\x1b[1;6D"
        );
        assert_eq!(
            encoded(with(KeyCode::End, KeyModifiers::CONTROL), normal()),
            b"\x1b[1;5F"
        );
    }

    #[test]
    fn the_navigation_keys_are_reported_by_number() {
        assert_eq!(encoded(key(KeyCode::Insert), normal()), b"\x1b[2~");
        assert_eq!(encoded(key(KeyCode::Delete), normal()), b"\x1b[3~");
        assert_eq!(encoded(key(KeyCode::PageUp), normal()), b"\x1b[5~");
        assert_eq!(encoded(key(KeyCode::PageDown), normal()), b"\x1b[6~");
        assert_eq!(
            encoded(with(KeyCode::Delete, KeyModifiers::SHIFT), normal()),
            b"\x1b[3;2~"
        );
    }

    #[test]
    fn the_first_four_function_keys_have_a_form_of_their_own() {
        assert_eq!(encoded(key(KeyCode::F(1)), normal()), b"\x1bOP");
        assert_eq!(encoded(key(KeyCode::F(2)), normal()), b"\x1bOQ");
        assert_eq!(encoded(key(KeyCode::F(3)), normal()), b"\x1bOR");
        assert_eq!(encoded(key(KeyCode::F(4)), normal()), b"\x1bOS");
        assert_eq!(
            encoded(with(KeyCode::F(1), KeyModifiers::SHIFT), normal()),
            b"\x1b[1;2P"
        );
    }

    #[test]
    fn the_remaining_function_keys_use_the_numbers_terminals_assigned() {
        for (number, code) in [
            (5_u8, 15_u16),
            (6, 17),
            (7, 18),
            (8, 19),
            (9, 20),
            (10, 21),
            (11, 23),
            (12, 24),
            (13, 25),
            (14, 26),
            (15, 28),
            (16, 29),
            (17, 31),
            (18, 32),
            (19, 33),
            (20, 34),
        ] {
            assert_eq!(
                encoded(key(KeyCode::F(number)), normal()),
                format!("\x1b[{code}~").as_bytes(),
                "F{number}"
            );
        }
        assert_eq!(encode(key(KeyCode::F(21)), normal()), None);
    }

    #[test]
    fn alt_puts_an_escape_in_front() {
        assert_eq!(
            encoded(with(KeyCode::Char('x'), KeyModifiers::ALT), normal()),
            b"\x1bx"
        );
        assert_eq!(
            encoded(with(KeyCode::Enter, KeyModifiers::ALT), normal()),
            b"\x1b\r"
        );
        // Alt with control keeps both: the escape, then the control byte.
        assert_eq!(
            encoded(
                with(
                    KeyCode::Char('c'),
                    KeyModifiers::ALT | KeyModifiers::CONTROL
                ),
                normal()
            ),
            [0x1b, 0x03]
        );
        // An arrow with alt keeps its mode-dependent form behind the escape.
        assert_eq!(
            encoded(with(KeyCode::Up, KeyModifiers::ALT), application()),
            b"\x1b\x1bOA"
        );
    }

    #[test]
    fn the_keypad_sends_its_own_sequences_only_when_a_program_asks() {
        let keypad = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('1'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
            KeyEventState::KEYPAD,
        );

        assert_eq!(encoded(keypad, application()), b"\x1bOq");
        // Without the mode, the keypad is just the character it prints.
        assert_eq!(encoded(keypad, normal()), b"1");
        // And the number row is never a keypad key.
        assert_eq!(encoded(key(KeyCode::Char('1')), application()), b"1");
    }

    #[test]
    fn a_key_release_is_not_input() {
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(encode(release, normal()), None);

        let repeat =
            KeyEvent::new_with_kind(KeyCode::Char('a'), KeyModifiers::NONE, KeyEventKind::Repeat);
        assert_eq!(encoded(repeat, normal()), b"a");
    }

    #[test]
    fn keys_otag_cannot_encode_send_nothing() {
        assert_eq!(encode(key(KeyCode::CapsLock), normal()), None);
        assert_eq!(encode(key(KeyCode::Menu), normal()), None);
        assert_eq!(encode(key(KeyCode::Null), normal()), None);
    }

    #[test]
    fn a_paste_is_plain_text_unless_a_program_asked_to_be_told() {
        assert_eq!(encode_paste("hello", normal()), b"hello");
        assert_eq!(
            encode_paste(
                "hello",
                InputModes {
                    bracketed_paste: true,
                    ..normal()
                }
            ),
            b"\x1b[200~hello\x1b[201~"
        );
    }

    #[test]
    fn a_pasted_newline_arrives_as_a_terminal_sends_it() {
        assert_eq!(encode_paste("one\ntwo", normal()), b"one\rtwo");
    }

    #[test]
    fn a_paste_cannot_end_itself_and_have_the_rest_treated_as_typing() {
        let modes = InputModes {
            bracketed_paste: true,
            ..normal()
        };
        let pasted = encode_paste("safe\x1b[201~rm -rf /", modes);

        assert_eq!(pasted, b"\x1b[200~saferm -rf /\x1b[201~".to_vec());
        // Exactly one end marker, at the end.
        let text = String::from_utf8(pasted).expect("the paste is text");
        assert_eq!(text.matches("\x1b[201~").count(), 1);
        assert!(text.ends_with("\x1b[201~"));
    }
}
