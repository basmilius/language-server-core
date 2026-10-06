# language-server-core

The shared foundation of the language servers of `basmilius/language-server-*`, in Rust: what every one of them does the same way, so that a server holds only what its language means. It is not on crates.io; a server depends on it through Git, pinned to a tag.

## Crates

| Crate        | Folder           | What it holds                                                                                                          |
| ------------ | ---------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `lsc-text`   | `crates/text`    | `LineIndex`, `LineCol` and `PositionEncoding`: byte offsets to lines and UTF-8, UTF-16 or UTF-32 columns and back       |
| `lsc-syntax` | `crates/syntax`  | The token cursor and tree builder an error-tolerant recursive descent parser on `rowan` is written on                   |
| `lsc-server` | `crates/server`  | `file:` URIs, open documents with incremental sync, encoding negotiation, dispatch, progress, the main loop and `main` |

`lsc-text` knows nothing of LSP, so a crate that answers questions about a tree can use it without the protocol. `lsc-syntax` re-exports `rowan` and `lsc-server` re-exports `lsp-server` and `lsp-types`, so a server builds on the same versions as the core.

## Using it

```toml
[workspace.dependencies]
lsc-server = { git = "https://github.com/basmilius/language-server-core", tag = "v0.1.0" }
lsc-syntax = { git = "https://github.com/basmilius/language-server-core", tag = "v0.1.0" }
lsc-text = { git = "https://github.com/basmilius/language-server-core", tag = "v0.1.0" }
```

A parser implements `TokenKind` for its kinds and drives `lsc_syntax::Parser`:

```rust
impl lsc_syntax::TokenKind for SyntaxKind {
    const EOF: SyntaxKind = SyntaxKind::EOF;
    const ERROR: SyntaxKind = SyntaxKind::ERROR;

    fn is_trivia(self) -> bool {
        matches!(self, SyntaxKind::WHITESPACE | SyntaxKind::COMMENT)
    }

    fn is_whitespace(self) -> bool {
        self == SyntaxKind::WHITESPACE
    }
}

pub type Parse = lsc_syntax::Parse<SqlLanguage>;

pub fn parse(text: &str) -> Parse {
    let mut p = lsc_syntax::Parser::<SqlLanguage>::new(text, lexer::lex(text));
    p.start_root(SyntaxKind::SOURCE_FILE);
    while !p.eof() {
        statement(&mut p);
    }
    p.flush_rest();
    p.finish_node();
    p.finish()
}
```

A server keeps its documents in `Documents<Parse, State>`, implements `Handler` and hands it to `main_loop`:

```rust
use lsc_server::lsp_server::{Connection, Notification, Request};
use lsc_server::lsp_types::request::{HoverRequest, Request as _};
use lsc_server::{BoxError, Client, Documents, Handler, PositionEncoding};

struct Server {
    client: Client,
    documents: Documents<Parse>,
    encoding: PositionEncoding,
}

impl Handler for Server {
    type Event = Event;

    fn request(&mut self, request: Request) -> Result<(), BoxError> {
        let response = match request.method.as_str() {
            HoverRequest::METHOD => lsc_server::answer(request.id, request.params, |params| self.hover(params)),
            method => lsc_server::unsupported(request.id, method),
        };
        self.client.send(response)
    }

    fn notification(&mut self, notification: Notification) -> Result<(), BoxError> {
        // `didOpen` opens with `self.documents.open(uri, version, text)`, `didChange` calls
        // `apply_changes(version, &changes, self.encoding)` on the document.
        Ok(())
    }

    fn event(&mut self, event: Event) -> Result<(), BoxError> {
        Ok(())
    }

    fn idle(&mut self) -> Result<(), BoxError> {
        // Publish the diagnostics of what changed, once per burst of changes.
        Ok(())
    }
}

pub fn run(connection: Connection) -> Result<(), BoxError> {
    let (id, params) = connection.initialize_start()?;
    // Pick the encoding with `lsc_server::choose_encoding`, answer with `encoding_kind`.
    connection.initialize_finish(id, result)?;
    let (_sender, events) = crossbeam_channel::unbounded();
    lsc_server::main_loop(&connection, &events, &mut server)
}

fn main() -> std::process::ExitCode {
    lsc_server::run_stdio("sql-language-server", env!("CARGO_PKG_VERSION"), USAGE, run)
}
```

A document parses lazily with `document.parse_with(parse)` and turns offsets into positions with `document.mapper(encoding)`. The `testing` feature of `lsc-text` has the `$0` cursor marker of tests, and that of `lsc-server` a `TestClient` that drives a server over an in-memory connection.

## Checks

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

## Releases

A change a server needs gets a new tag, `v<version>` with the version of `Cargo.toml`, and the server moves its pin to it. A tag never moves. [CLAUDE.md](./CLAUDE.md) says how to work on a server against a local checkout of this repository.

## License

[Functional Source License, Version 1.1, MIT Future License](./LICENSE).
