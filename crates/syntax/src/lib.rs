//! What an error-tolerant recursive descent parser on `rowan` is written on: a cursor over the
//! tokens a lexer made that steps over trivia, a tree builder that puts the trivia back where it
//! belongs, and the errors found on the way. The grammar, the kinds and the lexer are the
//! language's own.

mod parser;

pub use parser::{FUEL, MAX_DEPTH, Parse, Parser, SyntaxError, Token, TokenKind};
pub use rowan;
pub use rowan::{Checkpoint, TextRange, TextSize};
