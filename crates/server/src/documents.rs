//! The documents a client opened, synced with full or incremental changes.

use std::collections::HashMap;
use std::path::PathBuf;

use lsc_text::{LineCol, LineIndex, PositionEncoding};
use lsp_types::{TextDocumentContentChangeEvent, Uri};

use crate::convert::Mapper;
use crate::paths::uri_to_path;

/// An open document: its text, the tree of that text once something asked for it, and `S`, what
/// the server keeps per document besides.
pub struct Document<P, S = ()> {
    pub version: i32,
    pub text: String,
    pub index: LineIndex,
    pub state: S,
    parsed: Option<P>,
}

impl<P, S: Default> Document<P, S> {
    pub fn new(version: i32, text: String) -> Document<P, S> {
        let index = LineIndex::new(&text);
        Document {
            version,
            text,
            index,
            state: S::default(),
            parsed: None,
        }
    }
}

impl<P, S> Document<P, S> {
    /// The tree of the current text, parsed with `parse` the first time it is asked for after a
    /// change. The whole text is parsed again: for a file a person edits, that is cheaper than
    /// anything a patch would save, and a burst of changes costs one parse.
    pub fn parse_with(&mut self, parse: impl FnOnce(&str) -> P) -> &P {
        self.parsed.get_or_insert_with(|| parse(&self.text))
    }

    /// The tree of the text, when it was parsed since the last change.
    pub fn cached(&self) -> Option<&P> {
        self.parsed.as_ref()
    }

    /// Forgets the tree, so the next [`Self::parse_with`] parses again: for a server whose tree
    /// depends on a setting of the document besides its text.
    pub fn invalidate(&mut self) {
        self.parsed = None;
    }

    pub fn mapper(&self, encoding: PositionEncoding) -> Mapper<'_> {
        Mapper {
            text: &self.text,
            index: &self.index,
            encoding,
        }
    }

    /// Applies the changes of one `textDocument/didChange` in order. A change without a range
    /// replaces the text; the range of a change is read against the text the changes before it
    /// left, as LSP says, and a range given backwards is read forwards.
    pub fn apply_changes(
        &mut self,
        version: i32,
        changes: &[TextDocumentContentChangeEvent],
        encoding: PositionEncoding,
    ) {
        for change in changes {
            match change.range {
                None => self.text.clone_from(&change.text),
                Some(range) => {
                    let start = self.index.offset(
                        &self.text,
                        LineCol {
                            line: range.start.line,
                            col: range.start.character,
                        },
                        encoding,
                    );
                    let end = self.index.offset(
                        &self.text,
                        LineCol {
                            line: range.end.line,
                            col: range.end.character,
                        },
                        encoding,
                    );
                    let (start, end) = if end < start { (end, start) } else { (start, end) };
                    self.text
                        .replace_range(usize::from(start)..usize::from(end), &change.text);
                }
            }
            self.index = LineIndex::new(&self.text);
        }
        self.version = version;
        self.parsed = None;
    }
}

/// The open documents by URI.
pub struct Documents<P, S = ()> {
    open: HashMap<Uri, Document<P, S>>,
}

impl<P, S> Default for Documents<P, S> {
    fn default() -> Documents<P, S> {
        Documents { open: HashMap::new() }
    }
}

impl<P, S: Default> Documents<P, S> {
    /// Opens a document, or opens it again with this text when it was open.
    pub fn open(&mut self, uri: Uri, version: i32, text: String) -> &mut Document<P, S> {
        self.open.insert(uri.clone(), Document::new(version, text));
        self.open.get_mut(&uri).expect("inserted above")
    }
}

impl<P, S> Documents<P, S> {
    pub fn close(&mut self, uri: &Uri) -> Option<Document<P, S>> {
        self.open.remove(uri)
    }

    pub fn get(&self, uri: &Uri) -> Option<&Document<P, S>> {
        self.open.get(uri)
    }

    pub fn get_mut(&mut self, uri: &Uri) -> Option<&mut Document<P, S>> {
        self.open.get_mut(uri)
    }

    /// The text of every open `file:` document by path, for searches that must see unsaved changes.
    pub fn texts(&self) -> HashMap<PathBuf, String> {
        self.open
            .iter()
            .filter_map(|(uri, document)| Some((uri_to_path(uri)?, document.text.clone())))
            .collect()
    }

    pub fn uris(&self) -> Vec<Uri> {
        self.open.keys().cloned().collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Uri, &Document<P, S>)> {
        self.open.iter()
    }

    pub fn len(&self) -> usize {
        self.open.len()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use lsp_types::{Position, Range};

    type Plain = Document<usize>;

    fn change(start: (u32, u32), end: (u32, u32), text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1))),
            range_length: None,
            text: text.to_string(),
        }
    }

    fn whole(text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.to_string(),
        }
    }

    fn uri(text: &str) -> Uri {
        Uri::from_str(text).expect("a uri")
    }

    #[test]
    fn applies_incremental_changes_in_order() {
        let mut document = Plain::new(1, "<?php\n$a = 1;\n$b = 2;\n".to_string());
        document.apply_changes(
            2,
            &[change((1, 5), (1, 6), "42"), change((2, 0), (2, 0), "// x\n")],
            PositionEncoding::Utf16,
        );
        assert_eq!(document.text, "<?php\n$a = 42;\n// x\n$b = 2;\n");
        assert_eq!(document.version, 2);
    }

    #[test]
    fn reads_each_change_against_the_text_the_one_before_left() {
        let mut document = Plain::new(1, "ab".to_string());
        document.apply_changes(
            2,
            &[change((0, 2), (0, 2), "\ncd"), change((1, 2), (1, 2), "!")],
            PositionEncoding::Utf16,
        );
        assert_eq!(document.text, "ab\ncd!");
        assert_eq!(document.index.line_count(), 2);
    }

    #[test]
    fn counts_utf16_columns_after_astral_characters() {
        let mut utf16 = Plain::new(1, "<?php\n$a = '\u{1f600}x';\n".to_string());
        utf16.apply_changes(2, &[change((1, 8), (1, 9), "y")], PositionEncoding::Utf16);
        assert_eq!(utf16.text, "<?php\n$a = '\u{1f600}y';\n");
        let mut utf8 = Plain::new(1, "<?php\n$a = '\u{1f600}x';\n".to_string());
        utf8.apply_changes(2, &[change((1, 10), (1, 11), "y")], PositionEncoding::Utf8);
        assert_eq!(utf8.text, utf16.text);
        let mut utf32 = Plain::new(1, "<?php\n$a = '\u{1f600}x';\n".to_string());
        utf32.apply_changes(2, &[change((1, 7), (1, 8), "y")], PositionEncoding::Utf32);
        assert_eq!(utf32.text, utf16.text);
    }

    #[test]
    fn deletes_an_astral_character_whole() {
        let mut document = Plain::new(1, "a\u{1f600}b".to_string());
        document.apply_changes(2, &[change((0, 1), (0, 3), "")], PositionEncoding::Utf16);
        assert_eq!(document.text, "ab");
    }

    #[test]
    fn edits_across_crlf_line_breaks() {
        let mut document = Plain::new(1, "one\r\ntwo\r\nthree".to_string());
        document.apply_changes(2, &[change((0, 3), (1, 3), "")], PositionEncoding::Utf16);
        assert_eq!(document.text, "one\r\nthree");
        document.apply_changes(3, &[change((1, 0), (1, 0), "x\r\n")], PositionEncoding::Utf16);
        assert_eq!(document.text, "one\r\nx\r\nthree");
        assert_eq!(document.index.line_count(), 3);
        assert_eq!(document.index.line_start(2), 8);
    }

    #[test]
    fn a_change_past_the_end_appends() {
        let mut document = Plain::new(1, "ab".to_string());
        document.apply_changes(2, &[change((5, 0), (9, 9), "c")], PositionEncoding::Utf16);
        assert_eq!(document.text, "abc");
        document.apply_changes(3, &[change((0, 99), (0, 99), "d")], PositionEncoding::Utf16);
        assert_eq!(document.text, "abcd");
    }

    #[test]
    fn a_backwards_range_is_read_forwards() {
        let mut document = Plain::new(1, "abcdef".to_string());
        document.apply_changes(2, &[change((0, 4), (0, 1), "-")], PositionEncoding::Utf16);
        assert_eq!(document.text, "a-ef");
    }

    #[test]
    fn invalidating_parses_the_same_text_again() {
        let mut document = Plain::new(1, "abc".to_string());
        assert_eq!(*document.parse_with(str::len), 3);
        assert_eq!(
            *document.parse_with(|_| 0),
            3,
            "the tree is kept until something changes"
        );
        document.invalidate();
        assert!(document.cached().is_none());
        assert_eq!(*document.parse_with(|_| 7), 7);
        assert_eq!(document.text, "abc");
    }

    #[test]
    fn a_change_without_a_range_replaces_the_text() {
        let mut document = Plain::new(1, "<?php 1;".to_string());
        document.apply_changes(2, &[whole("<?php 2;\nnext")], PositionEncoding::Utf16);
        assert_eq!(document.text, "<?php 2;\nnext");
        assert_eq!(document.index.line_count(), 2);
    }

    #[test]
    fn a_full_change_then_an_incremental_one_reads_the_new_text() {
        let mut document = Plain::new(1, "old".to_string());
        document.apply_changes(
            2,
            &[whole("first\nsecond"), change((1, 0), (1, 6), "2nd")],
            PositionEncoding::Utf16,
        );
        assert_eq!(document.text, "first\n2nd");
    }

    #[test]
    fn parses_once_until_the_next_change() {
        let mut document = Plain::new(1, "abc".to_string());
        let mut calls = 0;
        assert_eq!(document.cached(), None);
        assert_eq!(
            *document.parse_with(|text| {
                calls += 1;
                text.len()
            }),
            3
        );
        document.parse_with(|_| {
            calls += 1;
            0
        });
        assert_eq!(calls, 1);
        assert_eq!(document.cached(), Some(&3));
        document.apply_changes(2, &[change((0, 0), (0, 0), "xy")], PositionEncoding::Utf16);
        assert_eq!(document.cached(), None);
        assert_eq!(*document.parse_with(str::len), 5);
    }

    #[test]
    fn keeps_the_state_of_a_document_across_changes() {
        let mut document: Document<(), Vec<&str>> = Document::new(1, String::new());
        document.state.push("kept");
        document.apply_changes(2, &[whole("x")], PositionEncoding::Utf16);
        assert_eq!(document.state, ["kept"]);
    }

    #[test]
    fn maps_with_the_text_and_index_of_the_document() {
        let document = Plain::new(1, "a\n\u{1f600}b".to_string());
        let mapper = document.mapper(PositionEncoding::Utf16);
        assert_eq!(mapper.position(lsc_text::TextSize::from(6)), Position::new(1, 2));
    }

    #[test]
    fn keeps_the_open_documents_by_uri() {
        let mut documents: Documents<usize> = Documents::default();
        assert!(documents.is_empty());
        let first = uri("file:///project/a%20b.sql");
        let second = uri("untitled:Untitled-1");
        documents.open(first.clone(), 1, "one".to_string());
        documents.open(second.clone(), 1, "two".to_string()).version = 7;
        assert_eq!(documents.len(), 2);
        assert_eq!(documents.get(&second).map(|document| document.version), Some(7));
        documents
            .get_mut(&first)
            .expect("open")
            .apply_changes(2, &[whole("uno")], PositionEncoding::Utf16);
        let texts = documents.texts();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[&PathBuf::from("/project/a b.sql")], "uno");
        let mut uris = documents.uris();
        uris.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        assert_eq!(uris, [first.clone(), second.clone()]);
        assert_eq!(documents.iter().count(), 2);
        assert_eq!(
            documents.close(&first).map(|document| document.text),
            Some("uno".to_string())
        );
        assert!(documents.get(&first).is_none());
        assert!(documents.close(&first).is_none());
    }

    #[test]
    fn opening_an_open_document_starts_it_over() {
        let mut documents: Documents<usize, u8> = Documents::default();
        let uri = uri("file:///a.sql");
        documents.open(uri.clone(), 1, "one".to_string()).state = 9;
        documents.open(uri.clone(), 3, "three".to_string());
        let document = documents.get(&uri).expect("open");
        assert_eq!(
            (document.version, document.text.as_str(), document.state),
            (3, "three", 0)
        );
    }
}
