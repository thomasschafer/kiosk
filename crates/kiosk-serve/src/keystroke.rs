//! Key presses as GPUI spells them (`ctrl-shift-a`, `escape`), turned into kiosk's own.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use kiosk_core::{
    config::keys::{KeyChord, function_key},
    keymap::KeyInput,
};

/// The modifier prefixes GPUI writes, in its order, and what each means to kiosk. GPUI
/// spells the platform key `cmd` on every platform; kiosk has no binding for it, so a chord
/// with it never resolves to a command.
const MODIFIERS: [(&str, KeyModifiers); 5] = [
    ("ctrl-", KeyModifiers::CONTROL),
    ("alt-", KeyModifiers::ALT),
    ("shift-", KeyModifiers::SHIFT),
    ("cmd-", KeyModifiers::SUPER),
    ("fn-", KeyModifiers::NONE),
];

const NAMED_KEYS: [(&str, KeyCode); 14] = [
    ("enter", KeyCode::Enter),
    ("escape", KeyCode::Esc),
    ("tab", KeyCode::Tab),
    ("backspace", KeyCode::Backspace),
    ("delete", KeyCode::Delete),
    ("up", KeyCode::Up),
    ("down", KeyCode::Down),
    ("left", KeyCode::Left),
    ("right", KeyCode::Right),
    ("home", KeyCode::Home),
    ("end", KeyCode::End),
    ("pageup", KeyCode::PageUp),
    ("pagedown", KeyCode::PageDown),
    ("space", KeyCode::Char(' ')),
];

/// Which platform's conventions decide whether a chord with alt types its character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Option composes characters, such as `option+2` for `@` on many layouts.
    MacOs,
    Other,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }
}

/// A key press from GPUI: `keystroke` as `Keystroke::unparse` spells it and `key_char`,
/// the text it would type. Returns `None` for keys kiosk has no name for, such as `f13`.
pub fn parse(keystroke: &str, key_char: Option<&str>, platform: Platform) -> Option<KeyInput> {
    let mut rest = keystroke;
    let mut modifiers = KeyModifiers::NONE;
    'prefixes: loop {
        for (prefix, modifier) in MODIFIERS {
            if let Some(after) = rest.strip_prefix(prefix)
                && !after.is_empty()
            {
                modifiers |= modifier;
                rest = after;
                continue 'prefixes;
            }
        }
        break;
    }
    let typed = single_char(key_char).filter(|character| !character.is_control());
    let code = if let Some((_, code)) = NAMED_KEYS.iter().find(|(name, _)| *name == rest) {
        *code
    } else if let Some(number) = function_key(rest) {
        KeyCode::F(number)
    } else {
        let key = single_char(Some(rest))?;
        match typed {
            // Shifted punctuation is bound by the character it types (`?`, not `shift+/`).
            Some(character)
                if modifiers.contains(KeyModifiers::SHIFT) && !key.is_ascii_alphabetic() =>
            {
                modifiers.remove(KeyModifiers::SHIFT);
                KeyCode::Char(character)
            }
            _ => KeyCode::Char(key),
        }
    };
    let chord = KeyChord::from_event(KeyEvent::new(code, modifiers));
    let chord_types_text = !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        && (!modifiers.contains(KeyModifiers::ALT) || platform == Platform::MacOs);
    Some(KeyInput {
        chord,
        text: typed.filter(|_| chord_types_text),
    })
}

fn single_char(value: Option<&str>) -> Option<char> {
    let mut chars = value?.chars();
    let character = chars.next()?;
    chars.next().is_none().then_some(character)
}

/// `chord` as GPUI spells it, for binding it. `None` for a character typed without ctrl, alt
/// or the platform key, which must arrive as a key press to carry the text it types, and
/// for keys GPUI has no spelling for.
pub fn unparse(chord: KeyChord) -> Option<String> {
    let mut spelled = String::new();
    for (prefix, modifier) in MODIFIERS {
        if !modifier.is_empty() && chord.modifiers.contains(modifier) {
            spelled.push_str(prefix);
        }
    }
    match chord.code {
        KeyCode::Char(_)
            if !chord
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            return None;
        }
        KeyCode::Char(' ') => spelled.push_str("space"),
        KeyCode::Char(character) => spelled.extend(character.to_lowercase()),
        KeyCode::F(number) => {
            let name = format!("f{number}");
            function_key(&name)?;
            spelled.push_str(&name);
        }
        code => {
            let (name, _) = NAMED_KEYS.iter().find(|(_, named)| *named == code)?;
            spelled.push_str(name);
        }
    }
    Some(spelled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(code: KeyCode, modifiers: KeyModifiers) -> KeyChord {
        KeyChord::new(code, modifiers)
    }

    fn input(keystroke: &str, key_char: Option<&str>) -> KeyInput {
        parse(keystroke, key_char, Platform::Other).unwrap()
    }

    #[test]
    fn named_keys_and_modifiers_become_chords() {
        assert_eq!(
            input("f1", None).chord,
            chord(KeyCode::F(1), KeyModifiers::NONE)
        );
        assert_eq!(
            input("escape", None).chord,
            chord(KeyCode::Esc, KeyModifiers::NONE)
        );
        assert_eq!(
            input("ctrl-h", None).chord,
            chord(KeyCode::Char('h'), KeyModifiers::CONTROL)
        );
        assert_eq!(
            input("alt-backspace", None).chord,
            chord(KeyCode::Backspace, KeyModifiers::ALT)
        );
        assert_eq!(
            input("ctrl-alt-shift-up", None).chord,
            chord(
                KeyCode::Up,
                KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT
            )
        );
        assert_eq!(
            input("shift-tab", None).chord,
            chord(KeyCode::Tab, KeyModifiers::SHIFT)
        );
    }

    #[test]
    fn a_minus_key_after_modifiers_is_the_key_itself() {
        assert_eq!(
            input("ctrl--", None).chord,
            chord(KeyCode::Char('-'), KeyModifiers::CONTROL)
        );
        assert_eq!(
            input("-", Some("-")).chord,
            chord(KeyCode::Char('-'), KeyModifiers::NONE)
        );
    }

    #[test]
    fn typed_characters_come_from_key_char() {
        let shifted = input("shift-a", Some("A"));
        assert_eq!(
            shifted.chord,
            chord(KeyCode::Char('A'), KeyModifiers::SHIFT)
        );
        assert_eq!(shifted.text, Some('A'));

        let question = input("shift-/", Some("?"));
        assert_eq!(
            question.chord,
            chord(KeyCode::Char('?'), KeyModifiers::NONE)
        );
        assert_eq!(question.text, Some('?'));

        assert_eq!(input("é", Some("é")).text, Some('é'));
        assert_eq!(input("space", Some(" ")).text, Some(' '));
    }

    #[test]
    fn control_and_platform_chords_type_nothing() {
        assert_eq!(input("ctrl-a", Some("a")).text, None);
        assert_eq!(input("cmd-a", Some("a")).text, None);
        assert_eq!(
            input("cmd-a", Some("a")).chord.modifiers,
            KeyModifiers::SUPER
        );
        assert_eq!(input("enter", Some("\r")).text, None);
    }

    #[test]
    fn option_types_its_character_only_on_macos() {
        assert_eq!(input("alt-2", Some("@")).text, None);
        let mac = parse("alt-2", Some("@"), Platform::MacOs).unwrap();
        assert_eq!(mac.text, Some('@'));
        assert_eq!(mac.chord, chord(KeyCode::Char('2'), KeyModifiers::ALT));
    }

    #[test]
    fn unknown_keys_are_refused() {
        assert_eq!(parse("f13", None, Platform::Other), None);
        assert_eq!(parse("insert", None, Platform::Other), None);
        assert_eq!(parse("", None, Platform::Other), None);
    }

    #[test]
    fn chords_spell_back_for_binding() {
        assert_eq!(
            unparse(chord(KeyCode::Char('h'), KeyModifiers::CONTROL)).as_deref(),
            Some("ctrl-h")
        );
        assert_eq!(
            unparse(chord(KeyCode::Esc, KeyModifiers::NONE)).as_deref(),
            Some("escape")
        );
        assert_eq!(
            unparse(chord(KeyCode::Backspace, KeyModifiers::ALT)).as_deref(),
            Some("alt-backspace")
        );
        assert_eq!(
            unparse(chord(
                KeyCode::Char('A'),
                KeyModifiers::SHIFT | KeyModifiers::CONTROL
            ))
            .as_deref(),
            Some("ctrl-shift-a")
        );
        assert_eq!(unparse(chord(KeyCode::Char('q'), KeyModifiers::NONE)), None);
        assert_eq!(
            unparse(chord(KeyCode::Char('Q'), KeyModifiers::SHIFT)),
            None
        );
        assert_eq!(unparse(chord(KeyCode::Char(' '), KeyModifiers::NONE)), None);
        assert_eq!(
            unparse(chord(KeyCode::Char(' '), KeyModifiers::CONTROL)).as_deref(),
            Some("ctrl-space")
        );
        assert_eq!(
            unparse(chord(KeyCode::F(5), KeyModifiers::NONE)).as_deref(),
            Some("f5")
        );
        assert_eq!(unparse(chord(KeyCode::F(13), KeyModifiers::NONE)), None);
    }

    #[test]
    fn every_default_binding_round_trips() {
        let keys = kiosk_core::config::keys::KeysConfig::default();
        for bound in keys.all_chords() {
            let Some(spelled) = unparse(bound) else {
                continue;
            };
            assert_eq!(input(&spelled, None).chord, bound, "{spelled}");
        }
    }
}
