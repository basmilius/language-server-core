//! Positions and ranges of LSP to offsets of a text and back, in the encoding the client chose.

use lsc_text::{LineCol, LineIndex, PositionEncoding, TextRange, TextSize};
use lsp_types::{Position, PositionEncodingKind, Range};

pub fn encoding_kind(encoding: PositionEncoding) -> PositionEncodingKind {
    match encoding {
        PositionEncoding::Utf8 => PositionEncodingKind::UTF8,
        PositionEncoding::Utf16 => PositionEncodingKind::UTF16,
        PositionEncoding::Utf32 => PositionEncodingKind::UTF32,
    }
}

/// The first of the client's encodings that the server prefers, or UTF-16 which every client has.
/// UTF-8 comes first since offsets are bytes, then UTF-32.
pub fn choose_encoding(offered: Option<&[PositionEncodingKind]>) -> PositionEncoding {
    let offered = offered.unwrap_or_default();
    [PositionEncodingKind::UTF8, PositionEncodingKind::UTF32]
        .iter()
        .find(|wanted| offered.contains(wanted))
        .map_or(PositionEncoding::Utf16, |found| {
            if *found == PositionEncodingKind::UTF8 {
                PositionEncoding::Utf8
            } else {
                PositionEncoding::Utf32
            }
        })
}

/// A text with its line index and the encoding of the client, to turn offsets into positions.
pub struct Mapper<'a> {
    pub text: &'a str,
    pub index: &'a LineIndex,
    pub encoding: PositionEncoding,
}

impl Mapper<'_> {
    pub fn position(&self, offset: TextSize) -> Position {
        let LineCol { line, col } = self.index.line_col(self.text, offset, self.encoding);
        Position::new(line, col)
    }

    pub fn range(&self, range: TextRange) -> Range {
        Range::new(self.position(range.start()), self.position(range.end()))
    }

    /// A range with something to draw: an empty one, where something is missing, widens to the
    /// character before it, or to the one after it at the start of a line.
    pub fn visible_range(&self, range: TextRange) -> Range {
        if !range.is_empty() {
            return self.range(range);
        }
        let offset = usize::from(range.start());
        if let Some(previous) = self.text[..offset]
            .chars()
            .next_back()
            .filter(|c| !matches!(c, '\n' | '\r'))
        {
            let start = TextSize::from((offset - previous.len_utf8()) as u32);
            return self.range(TextRange::new(start, range.start()));
        }
        if let Some(next) = self.text[offset..].chars().next().filter(|c| !matches!(c, '\n' | '\r')) {
            let end = TextSize::from((offset + next.len_utf8()) as u32);
            return self.range(TextRange::new(range.start(), end));
        }
        self.range(range)
    }

    pub fn offset(&self, position: Position) -> TextSize {
        self.index.offset(
            self.text,
            LineCol {
                line: position.line,
                col: position.character,
            },
            self.encoding,
        )
    }

    /// The offsets of a range.
    ///
    /// # Panics
    ///
    /// When the range ends before it starts.
    pub fn text_range(&self, range: Range) -> TextRange {
        TextRange::new(self.offset(range.start), self.offset(range.end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapper<'a>(text: &'a str, index: &'a LineIndex, encoding: PositionEncoding) -> Mapper<'a> {
        Mapper { text, index, encoding }
    }

    #[test]
    fn picks_utf8_when_offered_and_utf16_otherwise() {
        assert_eq!(choose_encoding(None), PositionEncoding::Utf16);
        assert_eq!(choose_encoding(Some(&[])), PositionEncoding::Utf16);
        assert_eq!(
            choose_encoding(Some(&[PositionEncodingKind::UTF16])),
            PositionEncoding::Utf16
        );
        assert_eq!(
            choose_encoding(Some(&[PositionEncodingKind::UTF16, PositionEncodingKind::UTF8])),
            PositionEncoding::Utf8
        );
        assert_eq!(
            choose_encoding(Some(&[PositionEncodingKind::UTF32])),
            PositionEncoding::Utf32
        );
        assert_eq!(
            choose_encoding(Some(&[PositionEncodingKind::UTF32, PositionEncodingKind::UTF8])),
            PositionEncoding::Utf8
        );
        assert_eq!(
            choose_encoding(Some(&[PositionEncodingKind::new("utf-7")])),
            PositionEncoding::Utf16
        );
    }

    #[test]
    fn names_every_encoding_as_lsp_does() {
        for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16, PositionEncoding::Utf32] {
            assert_eq!(choose_encoding(Some(&[encoding_kind(encoding)])), encoding);
        }
        assert_eq!(encoding_kind(PositionEncoding::Utf8).as_str(), "utf-8");
        assert_eq!(encoding_kind(PositionEncoding::Utf16).as_str(), "utf-16");
        assert_eq!(encoding_kind(PositionEncoding::Utf32).as_str(), "utf-32");
    }

    #[test]
    fn maps_positions_after_an_astral_character_in_each_encoding() {
        let text = "a\n\u{1f600}x\r\ny";
        let index = LineIndex::new(text);
        let x = TextSize::from(text.find('x').expect("an x") as u32);
        let expected = [
            (PositionEncoding::Utf8, 4),
            (PositionEncoding::Utf16, 2),
            (PositionEncoding::Utf32, 1),
        ];
        for (encoding, character) in expected {
            let mapper = mapper(text, &index, encoding);
            assert_eq!(mapper.position(x), Position::new(1, character));
            assert_eq!(mapper.offset(Position::new(1, character)), x);
        }
    }

    #[test]
    fn maps_ranges_both_ways() {
        let text = "one\r\ntwo three";
        let index = LineIndex::new(text);
        let mapper = mapper(text, &index, PositionEncoding::Utf16);
        let range = TextRange::new(TextSize::from(5), TextSize::from(8));
        let lsp = mapper.range(range);
        assert_eq!(lsp, Range::new(Position::new(1, 0), Position::new(1, 3)));
        assert_eq!(mapper.text_range(lsp), range);
    }

    #[test]
    fn widens_an_empty_range_to_a_character() {
        let text = "$a = ;\nx";
        let index = LineIndex::new(text);
        let mapper = mapper(text, &index, PositionEncoding::Utf16);
        let widened = mapper.visible_range(TextRange::empty(TextSize::from(5)));
        assert_eq!((widened.start.character, widened.end.character), (4, 5));
        let at_line_start = mapper.visible_range(TextRange::empty(TextSize::from(7)));
        assert_eq!(
            (
                at_line_start.start.line,
                at_line_start.start.character,
                at_line_start.end.character
            ),
            (1, 0, 1)
        );
    }

    #[test]
    fn widens_over_a_whole_astral_character() {
        let text = "\u{1f600}";
        let index = LineIndex::new(text);
        let mapper = mapper(text, &index, PositionEncoding::Utf16);
        let before = mapper.visible_range(TextRange::empty(TextSize::from(4)));
        assert_eq!(before, Range::new(Position::new(0, 0), Position::new(0, 2)));
        let after = mapper.visible_range(TextRange::empty(TextSize::from(0)));
        assert_eq!(after, Range::new(Position::new(0, 0), Position::new(0, 2)));
    }

    #[test]
    fn an_empty_range_on_an_empty_line_stays_empty() {
        let text = "a\n\nb";
        let index = LineIndex::new(text);
        let mapper = mapper(text, &index, PositionEncoding::Utf16);
        let range = mapper.visible_range(TextRange::empty(TextSize::from(2)));
        assert_eq!(range, Range::new(Position::new(1, 0), Position::new(1, 0)));
        let empty = LineIndex::new("");
        let mapper = Mapper {
            text: "",
            index: &empty,
            encoding: PositionEncoding::Utf16,
        };
        assert_eq!(
            mapper.visible_range(TextRange::empty(TextSize::from(0))),
            Range::default()
        );
    }

    #[test]
    fn a_nonempty_range_is_drawn_as_it_is() {
        let text = "abc";
        let index = LineIndex::new(text);
        let mapper = mapper(text, &index, PositionEncoding::Utf16);
        let range = TextRange::new(TextSize::from(1), TextSize::from(2));
        assert_eq!(mapper.visible_range(range), mapper.range(range));
    }
}
