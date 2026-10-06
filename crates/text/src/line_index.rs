use text_size::{TextRange, TextSize};

/// How the columns of a position count characters, as LSP lets client and server agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PositionEncoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

/// A zero-based line and a column in the units of a [`PositionEncoding`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

/// Maps byte offsets of a text to lines and columns and back. A line ends after `\n`, `\r\n` or a
/// lone `\r`, as LSP counts them.
#[derive(Clone, Debug)]
pub struct LineIndex {
    line_starts: Vec<u32>,
    len: u32,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut line_starts = vec![0];
        let bytes = text.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'\n' => line_starts.push(index as u32 + 1),
                b'\r' if bytes.get(index + 1) != Some(&b'\n') => line_starts.push(index as u32 + 1),
                _ => {}
            }
            index += 1;
        }
        LineIndex {
            line_starts,
            len: text.len() as u32,
        }
    }

    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }

    /// The byte offset where a line starts, or the end of the text for a line past it.
    pub fn line_start(&self, line: u32) -> u32 {
        self.line_starts.get(line as usize).copied().unwrap_or(self.len)
    }

    pub fn line_of(&self, offset: u32) -> u32 {
        match self.line_starts.binary_search(&offset) {
            Ok(line) => line as u32,
            Err(next) => next as u32 - 1,
        }
    }

    /// The line and column of an offset, which must fall on a character boundary of `text`. An
    /// offset past the end lands on the end.
    pub fn line_col(&self, text: &str, offset: TextSize, encoding: PositionEncoding) -> LineCol {
        let offset = u32::from(offset).min(self.len);
        let line = self.line_of(offset);
        let start = self.line_start(line);
        let col = match encoding {
            PositionEncoding::Utf8 => offset - start,
            PositionEncoding::Utf16 => text[start as usize..offset as usize]
                .chars()
                .map(|c| c.len_utf16() as u32)
                .sum(),
            PositionEncoding::Utf32 => text[start as usize..offset as usize].chars().count() as u32,
        };
        LineCol { line, col }
    }

    /// The offset of a position. A column past the end of its line lands on the line's end, and a
    /// line past the text on the end of the text, which is how LSP wants a bad position read. A
    /// column inside a character lands before that character.
    pub fn offset(&self, text: &str, position: LineCol, encoding: PositionEncoding) -> TextSize {
        if position.line >= self.line_count() {
            return TextSize::from(self.len);
        }
        let start = self.line_start(position.line) as usize;
        let end = self.line_start(position.line + 1) as usize;
        let line = &text[start..end];
        let line_without_break = line.trim_end_matches(['\n', '\r']);
        let mut remaining = position.col;
        let mut offset = 0usize;
        for character in line_without_break.chars() {
            let width = match encoding {
                PositionEncoding::Utf8 => character.len_utf8() as u32,
                PositionEncoding::Utf16 => character.len_utf16() as u32,
                PositionEncoding::Utf32 => 1,
            };
            if remaining < width {
                break;
            }
            remaining -= width;
            offset += character.len_utf8();
        }
        TextSize::from((start + offset) as u32)
    }

    pub fn range(&self, text: &str, range: TextRange, encoding: PositionEncoding) -> (LineCol, LineCol) {
        (
            self.line_col(text, range.start(), encoding),
            self.line_col(text, range.end(), encoding),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENCODINGS: [PositionEncoding; 3] = [PositionEncoding::Utf8, PositionEncoding::Utf16, PositionEncoding::Utf32];

    fn at(line: u32, col: u32) -> LineCol {
        LineCol { line, col }
    }

    fn offset_of(text: &str, needle: &str) -> TextSize {
        TextSize::from(text.find(needle).expect("the needle is in the text") as u32)
    }

    #[test]
    fn utf16_is_the_default_encoding() {
        assert_eq!(PositionEncoding::default(), PositionEncoding::Utf16);
    }

    #[test]
    fn converts_between_offsets_and_positions_in_every_encoding() {
        let text = "ab\nc\u{e9}\u{1f600}d\r\nlast";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 3);
        let after_emoji = offset_of(text, "d");
        let utf16 = index.line_col(text, after_emoji, PositionEncoding::Utf16);
        assert_eq!((utf16.line, utf16.col), (1, 4));
        let utf8 = index.line_col(text, after_emoji, PositionEncoding::Utf8);
        assert_eq!(utf8.col, 1 + 2 + 4);
        let utf32 = index.line_col(text, after_emoji, PositionEncoding::Utf32);
        assert_eq!(utf32.col, 3);
        for encoding in ENCODINGS {
            let position = index.line_col(text, after_emoji, encoding);
            assert_eq!(index.offset(text, position, encoding), after_emoji);
        }
    }

    #[test]
    fn every_character_boundary_round_trips_in_every_encoding() {
        let text = "\u{1f600}a\r\n\u{e9}\u{10348}\rb\n\n\u{4e2d}\u{1f468}\u{200d}\u{1f469}z";
        let index = LineIndex::new(text);
        for (offset, _) in text.char_indices().chain([(text.len(), ' ')]) {
            // No position names the point between the two characters of a CRLF.
            if text[..offset].ends_with('\r') && text[offset..].starts_with('\n') {
                continue;
            }
            let offset = TextSize::from(offset as u32);
            for encoding in ENCODINGS {
                let position = index.line_col(text, offset, encoding);
                assert_eq!(
                    index.offset(text, position, encoding),
                    offset,
                    "{encoding:?} at {offset:?}"
                );
            }
        }
    }

    #[test]
    fn counts_astral_characters_as_two_utf16_units() {
        let text = "\u{1f600}\u{1f600}x";
        let index = LineIndex::new(text);
        let x = offset_of(text, "x");
        assert_eq!(index.line_col(text, x, PositionEncoding::Utf8), at(0, 8));
        assert_eq!(index.line_col(text, x, PositionEncoding::Utf16), at(0, 4));
        assert_eq!(index.line_col(text, x, PositionEncoding::Utf32), at(0, 2));
    }

    #[test]
    fn a_column_inside_a_character_lands_before_it() {
        let text = "a\u{1f600}b";
        let index = LineIndex::new(text);
        let emoji = TextSize::from(1);
        assert_eq!(index.offset(text, at(0, 2), PositionEncoding::Utf16), emoji);
        assert_eq!(index.offset(text, at(0, 3), PositionEncoding::Utf8), emoji);
        assert_eq!(index.offset(text, at(0, 3), PositionEncoding::Utf16), TextSize::from(5));
    }

    #[test]
    fn crlf_is_one_line_break_and_its_columns_stop_before_it() {
        let text = "ab\r\ncd\r\n";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 3);
        assert_eq!(index.line_start(1), 4);
        assert_eq!(index.line_start(2), 8);
        for encoding in ENCODINGS {
            assert_eq!(index.offset(text, at(0, 99), encoding), TextSize::from(2));
            assert_eq!(index.offset(text, at(1, 2), encoding), TextSize::from(6));
            assert_eq!(index.line_col(text, TextSize::from(8), encoding), at(2, 0));
        }
    }

    #[test]
    fn the_carriage_return_of_a_crlf_belongs_to_its_line() {
        let text = "ab\r\ncd";
        let index = LineIndex::new(text);
        assert_eq!(
            index.line_col(text, TextSize::from(3), PositionEncoding::Utf16),
            at(0, 3)
        );
        assert_eq!(index.line_of(3), 0);
        assert_eq!(index.line_of(4), 1);
    }

    #[test]
    fn a_lone_carriage_return_breaks_a_line() {
        let text = "a\rb\r\nc";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 3);
        assert_eq!(
            index.line_col(text, offset_of(text, "b"), PositionEncoding::Utf16),
            at(1, 0)
        );
        assert_eq!(
            index.offset(text, at(2, 0), PositionEncoding::Utf16),
            offset_of(text, "c")
        );
    }

    #[test]
    fn clamps_positions_past_the_end() {
        let text = "ab\ncd";
        let index = LineIndex::new(text);
        assert_eq!(
            index.offset(text, at(0, 99), PositionEncoding::Utf16),
            TextSize::from(2)
        );
        assert_eq!(index.offset(text, at(9, 0), PositionEncoding::Utf16), TextSize::from(5));
        assert_eq!(
            index.offset(text, at(1, 99), PositionEncoding::Utf16),
            TextSize::from(5)
        );
        assert_eq!(
            index.line_col(text, TextSize::from(99), PositionEncoding::Utf16),
            at(1, 2)
        );
    }

    #[test]
    fn a_text_that_ends_in_a_line_break_has_an_empty_last_line() {
        let text = "a\n";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 2);
        assert_eq!(
            index.line_col(text, TextSize::from(2), PositionEncoding::Utf16),
            at(1, 0)
        );
        assert_eq!(index.offset(text, at(1, 0), PositionEncoding::Utf16), TextSize::from(2));
    }

    #[test]
    fn an_empty_text_has_one_empty_line() {
        let index = LineIndex::new("");
        assert_eq!(index.line_count(), 1);
        assert_eq!(index.line_start(0), 0);
        assert_eq!(index.line_start(5), 0);
        assert_eq!(index.line_col("", TextSize::from(0), PositionEncoding::Utf16), at(0, 0));
        assert_eq!(index.offset("", at(3, 3), PositionEncoding::Utf16), TextSize::from(0));
    }

    #[test]
    fn finds_the_line_of_an_offset() {
        let index = LineIndex::new("ab\ncd\n\nef");
        let lines: Vec<u32> = (0..=9).map(|offset| index.line_of(offset)).collect();
        assert_eq!(lines, [0, 0, 0, 1, 1, 1, 2, 3, 3, 3]);
    }

    #[test]
    fn maps_both_ends_of_a_range() {
        let text = "one\ntwo \u{1f600} three";
        let index = LineIndex::new(text);
        let start = offset_of(text, "two");
        let end = offset_of(text, " three");
        let (from, to) = index.range(text, TextRange::new(start, end), PositionEncoding::Utf16);
        assert_eq!((from, to), (at(1, 0), at(1, 6)));
    }
}
