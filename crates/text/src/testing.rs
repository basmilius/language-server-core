//! Markers that tests write into a text to point at a place in it.

/// Where a test puts the cursor. It cannot be mistaken for code in the languages built on this,
/// where a `$` is followed by a name or a nonzero number.
pub const CURSOR: &str = "$0";

/// The byte offset of the first [`CURSOR`] in a text and the text without it.
///
/// # Panics
///
/// When the text holds no cursor, which is a mistake in the test.
pub fn cursor(text: &str) -> (u32, String) {
    let offset = text.find(CURSOR).expect("the text holds a cursor marker");
    (offset as u32, text.replacen(CURSOR, "", 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_the_cursor_out_of_the_text() {
        assert_eq!(cursor("ab$0cd"), (2, "abcd".to_string()));
        assert_eq!(cursor("$0"), (0, String::new()));
    }

    #[test]
    fn only_the_first_marker_is_the_cursor() {
        assert_eq!(cursor("a$0b$0"), (1, "ab$0".to_string()));
    }

    #[test]
    fn counts_bytes_before_the_cursor() {
        assert_eq!(cursor("\u{1f600}$0x"), (4, "\u{1f600}x".to_string()));
    }

    #[test]
    #[should_panic(expected = "cursor marker")]
    fn a_text_without_a_cursor_is_a_mistake() {
        cursor("abc");
    }
}
