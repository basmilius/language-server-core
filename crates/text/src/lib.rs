//! Where a byte offset of a text sits as a line and a column, and back, in the units a client and
//! a server agree on. Nothing here knows about LSP, so a crate that answers questions about a tree
//! can use it without the protocol.

mod line_index;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use line_index::{LineCol, LineIndex, PositionEncoding};
pub use text_size::{TextRange, TextSize};
