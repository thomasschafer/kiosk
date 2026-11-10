use std::borrow::Cow;

pub fn sanitize(value: &str) -> Cow<'_, str> {
    if !value.chars().any(char::is_control) {
        return Cow::Borrowed(value);
    }

    let mut sanitized = String::with_capacity(value.len());
    for character in value.chars() {
        sanitized.extend(shown(character));
    }
    Cow::Owned(sanitized)
}

/// The characters of `value` at `positions` (character indices, ascending), as positions in
/// `sanitize(value)`: a control character's is where its escape starts.
pub fn sanitized_positions(value: &str, positions: &[usize]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(positions.len());
    let mut wanted = positions.iter().copied().peekable();
    let mut shown_at = 0;
    for (index, character) in value.chars().enumerate() {
        if wanted.next_if_eq(&index).is_some() {
            starts.push(shown_at);
        }
        shown_at += shown(character).count();
    }
    assert!(
        wanted.peek().is_none(),
        "positions {positions:?} aren't ascending character indices of {value:?}"
    );
    starts
}

/// Columns between tab stops when a line of code shows tabs as spaces.
pub const TAB_WIDTH: usize = 4;

/// A line of code as shown in a column grid: tabs as spaces to the next tab stop, control
/// characters as visible escapes and a final carriage return (a CRLF line ending) dropped.
/// Also how many shown characters each of the line's characters became.
pub fn code_chars(text: &str) -> (Vec<char>, Vec<usize>) {
    let text = text.strip_suffix('\r').unwrap_or(text);
    let mut chars: Vec<char> = Vec::with_capacity(text.len());
    let mut widths = Vec::with_capacity(text.len());
    for character in text.chars() {
        let before = chars.len();
        if character == '\t' {
            chars.extend(std::iter::repeat_n(' ', TAB_WIDTH - before % TAB_WIDTH));
        } else {
            chars.extend(shown(character));
        }
        widths.push(chars.len() - before);
    }
    (chars, widths)
}

/// How `sanitize` shows one character: a control character as its visible escape.
fn shown(character: char) -> impl Iterator<Item = char> {
    let control = character.is_control();
    control
        .then(|| character.escape_default())
        .into_iter()
        .flatten()
        .chain((!control).then_some(character))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_are_rendered_as_visible_escapes() {
        assert_eq!(
            sanitize("repo\nname\t\u{1b}").as_ref(),
            "repo\\nname\\t\\u{1b}"
        );
        assert!(matches!(sanitize("plain"), Cow::Borrowed("plain")));
    }

    #[test]
    fn positions_follow_the_escapes_before_them() {
        // "a\nb" shows as `a\nb`, four characters, so `b` moves from 2 to 3.
        assert_eq!(sanitized_positions("a\nb", &[0, 1, 2]), vec![0, 1, 3]);
        assert_eq!(sanitized_positions("plain", &[1, 4]), vec![1, 4]);
        assert_eq!(sanitized_positions("plain", &[]), Vec::<usize>::new());
    }

    #[test]
    #[should_panic(expected = "aren't ascending character indices")]
    fn positions_past_the_end_are_refused() {
        sanitized_positions("ab", &[5]);
    }
}
