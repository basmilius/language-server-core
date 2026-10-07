use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;

use rowan::{Checkpoint, GreenNode, GreenNodeBuilder, Language, SyntaxNode, TextRange, TextSize};

/// What the parser needs to know about the kinds of a language besides what `rowan` knows.
pub trait TokenKind: Copy + Eq {
    /// What the parser sees past the last token. Never part of a tree.
    const EOF: Self;
    /// The node around what the parser could not make sense of.
    const ERROR: Self;

    /// Whitespace and comments, which the parser keeps in the tree but never looks at.
    fn is_trivia(self) -> bool;

    /// Trivia that may stand between a doc comment and the declaration that takes it along.
    fn is_whitespace(self) -> bool;

    /// A comment that belongs to the declaration right after it. See
    /// [`Parser::start_before_declaration`].
    fn is_doc_comment(self) -> bool {
        false
    }
}

/// One token: its kind and its length in bytes. Offsets follow from the tokens before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token<K> {
    pub kind: K,
    pub len: u32,
}

/// A syntax error with the range it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub range: TextRange,
    pub message: String,
}

/// The tree of a text and the syntax errors found while building it.
pub struct Parse<L> {
    green: GreenNode,
    errors: Vec<SyntaxError>,
    language: PhantomData<L>,
}

impl<L: Language> Parse<L> {
    pub fn syntax(&self) -> SyntaxNode<L> {
        SyntaxNode::new_root(self.green.clone())
    }

    pub fn green(&self) -> &GreenNode {
        &self.green
    }

    pub fn errors(&self) -> &[SyntaxError] {
        &self.errors
    }
}

// Derived, these would ask the marker type of the language for the same traits.
impl<L> Clone for Parse<L> {
    fn clone(&self) -> Parse<L> {
        Parse {
            green: self.green.clone(),
            errors: self.errors.clone(),
            language: PhantomData,
        }
    }
}

impl<L> fmt::Debug for Parse<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Parse")
            .field("green", &self.green)
            .field("errors", &self.errors)
            .finish()
    }
}

/// How many lookups without consuming a token the parser allows before it gives up on the rest of
/// the file. Every loop of a grammar consumes a token or leaves, so this is a guard against a bug
/// and not a limit a real file reaches.
pub const FUEL: u32 = 200_000;

/// How deeply a grammar may nest before [`Parser::enter`] says no.
pub const MAX_DEPTH: u32 = 200;

/// A cursor over the significant tokens of a text and the builder of its tree. Trivia between
/// tokens goes into the tree as the cursor passes it, outside the node that starts next.
pub struct Parser<'a, L: Language> {
    text: &'a str,
    tokens: Vec<Token<L::Kind>>,
    starts: Vec<u32>,
    /// Indexes into `tokens` of everything that is not trivia.
    significant: Vec<u32>,
    cursor: usize,
    /// The next token that goes into the tree; trivia between it and the cursor waits.
    emitted: usize,
    builder: GreenNodeBuilder<'static>,
    errors: Vec<SyntaxError>,
    fuel: Cell<u32>,
    depth: u32,
}

impl<'a, L> Parser<'a, L>
where
    L: Language,
    L::Kind: TokenKind,
{
    /// A parser over the tokens of `text`, which together cover every byte of it.
    pub fn new(text: &'a str, tokens: Vec<Token<L::Kind>>) -> Parser<'a, L> {
        let mut starts = Vec::with_capacity(tokens.len());
        let mut significant = Vec::with_capacity(tokens.len());
        let mut offset = 0u32;
        for (index, token) in tokens.iter().enumerate() {
            starts.push(offset);
            offset += token.len;
            if !token.kind.is_trivia() {
                significant.push(index as u32);
            }
        }
        Parser {
            text,
            tokens,
            starts,
            significant,
            cursor: 0,
            emitted: 0,
            builder: GreenNodeBuilder::new(),
            errors: Vec::new(),
            fuel: Cell::new(FUEL),
            depth: 0,
        }
    }

    pub fn finish(self) -> Parse<L> {
        Parse {
            green: self.builder.finish(),
            errors: self.errors,
            language: PhantomData,
        }
    }

    pub fn text(&self) -> &'a str {
        self.text
    }

    // Nesting

    /// Counts one more level of nesting. Says no past [`MAX_DEPTH`], so deeply nested input ends
    /// in an error and not in a stack overflow. Every `true` is matched by a `leave`.
    pub fn enter(&mut self) -> bool {
        if self.depth >= MAX_DEPTH {
            if self.depth == MAX_DEPTH {
                self.depth += 1;
                self.error_here("Nesting is too deep");
            }
            return false;
        }
        self.depth += 1;
        true
    }

    pub fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    // Lookahead

    pub fn nth(&self, n: usize) -> L::Kind {
        let fuel = self.fuel.get();
        if fuel == 0 {
            return L::Kind::EOF;
        }
        self.fuel.set(fuel - 1);
        match self.significant.get(self.cursor + n) {
            Some(index) => self.tokens[*index as usize].kind,
            None => L::Kind::EOF,
        }
    }

    pub fn current(&self) -> L::Kind {
        self.nth(0)
    }

    pub fn at(&self, kind: L::Kind) -> bool {
        self.current() == kind
    }

    pub fn at_any(&self, kinds: &[L::Kind]) -> bool {
        kinds.contains(&self.current())
    }

    /// The index of the cursor, to tell whether a parse step consumed anything.
    pub fn position(&self) -> usize {
        self.cursor
    }

    pub fn eof(&self) -> bool {
        self.current() == L::Kind::EOF
    }

    /// The text of the `n`th upcoming token.
    pub fn nth_text(&self, n: usize) -> &'a str {
        match self.significant.get(self.cursor + n) {
            Some(index) => {
                let index = *index as usize;
                let start = self.starts[index] as usize;
                &self.text[start..start + self.tokens[index].len as usize]
            }
            None => "",
        }
    }

    pub fn current_text(&self) -> &'a str {
        self.nth_text(0)
    }

    /// Whether the `n`th upcoming token is followed by the next one with nothing in between.
    pub fn nth_touches_next(&self, n: usize) -> bool {
        match (
            self.significant.get(self.cursor + n),
            self.significant.get(self.cursor + n + 1),
        ) {
            (Some(first), Some(second)) => *first + 1 == *second,
            _ => false,
        }
    }

    /// Whether the current token follows the previous one with nothing in between.
    pub fn nth_touches_prev(&self) -> bool {
        match self
            .cursor
            .checked_sub(1)
            .and_then(|previous| self.significant.get(previous))
        {
            Some(previous) => self
                .significant
                .get(self.cursor)
                .is_some_and(|current| *previous + 1 == *current),
            None => false,
        }
    }

    /// The byte of the text before `offset`.
    pub fn byte_before(&self, offset: u32) -> Option<u8> {
        offset
            .checked_sub(1)
            .and_then(|at| self.text.as_bytes().get(at as usize))
            .copied()
    }

    /// The offset where the current token starts.
    pub fn current_offset(&self) -> u32 {
        self.significant
            .get(self.cursor)
            .map_or(self.text.len() as u32, |index| self.starts[*index as usize])
    }

    /// The range of the current token, or an empty range at the end of the text.
    pub fn current_range(&self) -> TextRange {
        match self.significant.get(self.cursor) {
            Some(index) => {
                let index = *index as usize;
                let start = self.starts[index];
                TextRange::new(TextSize::from(start), TextSize::from(start + self.tokens[index].len))
            }
            None => {
                let end = TextSize::of(self.text);
                TextRange::new(end, end)
            }
        }
    }

    /// An empty range right after the previous token, where something that is missing belongs.
    pub fn after_previous_range(&self) -> TextRange {
        let end = match self
            .cursor
            .checked_sub(1)
            .and_then(|previous| self.significant.get(previous))
        {
            Some(index) => {
                let index = *index as usize;
                self.starts[index] + self.tokens[index].len
            }
            None => self
                .significant
                .get(self.cursor)
                .map_or(self.text.len() as u32, |index| self.starts[*index as usize]),
        };
        TextRange::empty(TextSize::from(end))
    }

    // Building the tree

    fn flush_trivia(&mut self) {
        let until = self
            .significant
            .get(self.cursor)
            .map_or(self.tokens.len(), |index| *index as usize);
        while self.emitted < until {
            self.emit(self.emitted);
            self.emitted += 1;
        }
    }

    fn emit(&mut self, index: usize) {
        let token = self.tokens[index];
        let start = self.starts[index] as usize;
        self.builder.token(
            L::kind_to_raw(token.kind),
            &self.text[start..start + token.len as usize],
        );
    }

    /// Like `flush_trivia`, but a doc comment right before the next token stays out of the flush
    /// so the declaration that starts there takes it along.
    fn flush_trivia_before_declaration(&mut self) {
        let until = self
            .significant
            .get(self.cursor)
            .map_or(self.tokens.len(), |index| *index as usize);
        let mut doc = None;
        for index in (self.emitted..until).rev() {
            let kind = self.tokens[index].kind;
            if kind.is_whitespace() {
                continue;
            }
            if kind.is_doc_comment() {
                doc = Some(index);
            }
            break;
        }
        let stop = doc.unwrap_or(until);
        while self.emitted < stop {
            self.emit(self.emitted);
            self.emitted += 1;
        }
    }

    /// Starts a node at the next token. Trivia before it stays outside.
    pub fn start(&mut self, kind: L::Kind) {
        self.flush_trivia();
        self.builder.start_node(L::kind_to_raw(kind));
    }

    /// Starts a node whose first child may be a declaration, which takes the doc comment before it
    /// along instead of leaving it outside the node.
    pub fn start_before_declaration(&mut self, kind: L::Kind) {
        self.flush_trivia_before_declaration();
        self.builder.start_node(L::kind_to_raw(kind));
    }

    /// Starts the root, which also owns the trivia before the first token.
    pub fn start_root(&mut self, kind: L::Kind) {
        self.builder.start_node(L::kind_to_raw(kind));
    }

    pub fn finish_node(&mut self) {
        self.builder.finish_node();
    }

    pub fn checkpoint(&mut self) -> Checkpoint {
        self.flush_trivia();
        self.builder.checkpoint()
    }

    /// Like [`Self::checkpoint`], for a node that starts with a declaration and takes the doc
    /// comment before it.
    pub fn declaration_checkpoint(&mut self) -> Checkpoint {
        self.flush_trivia_before_declaration();
        self.builder.checkpoint()
    }

    pub fn start_at(&mut self, checkpoint: Checkpoint, kind: L::Kind) {
        self.builder.start_node_at(checkpoint, L::kind_to_raw(kind));
    }

    /// Consumes the current token into the tree.
    pub fn bump(&mut self) {
        if self.cursor >= self.significant.len() {
            return;
        }
        self.flush_trivia();
        self.emit(self.emitted);
        self.emitted += 1;
        self.cursor += 1;
        self.fuel.set(FUEL);
    }

    /// Consumes the current token into the tree as `kind`, for a word whose meaning the grammar
    /// decides: a keyword used as a name, or a name used as a keyword.
    pub fn bump_remap(&mut self, kind: L::Kind) {
        if self.cursor >= self.significant.len() {
            return;
        }
        self.flush_trivia();
        let index = self.emitted;
        let start = self.starts[index] as usize;
        self.builder.token(
            L::kind_to_raw(kind),
            &self.text[start..start + self.tokens[index].len as usize],
        );
        self.emitted += 1;
        self.cursor += 1;
        self.fuel.set(FUEL);
    }

    pub fn eat(&mut self, kind: L::Kind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Consumes `kind` or reports it missing, in which case nothing is consumed.
    pub fn expect(&mut self, kind: L::Kind, what: &str) -> bool {
        if self.eat(kind) {
            return true;
        }
        self.error_expected(what);
        false
    }

    /// Everything that is left of the text, trivia included, goes in before the root closes.
    pub fn flush_rest(&mut self) {
        while self.emitted < self.tokens.len() {
            self.emit(self.emitted);
            self.emitted += 1;
        }
    }

    // Errors

    pub fn error_at(&mut self, range: TextRange, message: impl Into<String>) {
        self.errors.push(SyntaxError {
            range,
            message: message.into(),
        });
    }

    /// Reports `what` missing right after the previous token, as `{what} expected`.
    pub fn error_expected(&mut self, what: &str) {
        let range = self.after_previous_range();
        self.errors.push(SyntaxError {
            range,
            message: format!("{what} expected"),
        });
    }

    pub fn error_here(&mut self, message: impl Into<String>) {
        let range = self.current_range();
        self.errors.push(SyntaxError {
            range,
            message: message.into(),
        });
    }

    /// Wraps the current token in an `ERROR` node with a message about it.
    pub fn error_bump(&mut self) {
        if self.current() == L::Kind::EOF {
            return;
        }
        let message = format!("Unexpected '{}'", self.current_text().escape_debug());
        self.error_here(message);
        self.start(L::Kind::ERROR);
        self.bump();
        self.finish_node();
    }

    /// Wraps tokens in one `ERROR` node until one of `stop` or the end of the text, taking at least one.
    pub fn error_recover(&mut self, stop: &[L::Kind]) {
        if self.eof() {
            return;
        }
        self.error_here(format!("Unexpected '{}'", self.current_text().escape_debug()));
        self.start(L::Kind::ERROR);
        self.bump();
        while !self.eof() && !stop.contains(&self.current()) {
            self.bump();
        }
        self.finish_node();
    }
}

#[cfg(test)]
mod tests;
