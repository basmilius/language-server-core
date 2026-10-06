//! What every language server over LSP does the same way: `file:` URIs and paths, the open
//! documents and their incremental changes, the position encoding a client and the server agree
//! on, the requests the server sends the client, work done progress, the dispatch of a request to
//! a handler, the main loop and the process around it. What a language means stays with its server.

mod client;
mod convert;
mod dispatch;
mod documents;
mod main_loop;
pub mod paths;
mod progress;
mod stdio;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use client::Client;
pub use convert::{Mapper, choose_encoding, encoding_kind};
pub use dispatch::{answer, answer_checked, unsupported};
pub use documents::{Document, Documents};
pub use lsc_text::{LineCol, LineIndex, PositionEncoding, TextRange, TextSize};
pub use lsp_server;
pub use lsp_types;
pub use main_loop::{Handler, main_loop};
pub use progress::{Progress, ProgressLabels};
pub use stdio::run_stdio;

/// Any error that ends the server.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
