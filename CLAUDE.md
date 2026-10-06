# basmilius/language-server-core

The shared foundation of the language servers in Rust: line index and position encodings, the parser infrastructure on `rowan`, and the LSP plumbing every server does the same way. `README.md` says what each crate holds and how a server wires it, and this file is for agents who work on it.

## Who uses it

- `basmilius/language-server-php` (a checkout usually sits beside this one as `../php`).
- The SQL server, built on it next (`../sql`).

A server depends on it through Git pinned to a tag, never through crates.io and never on a branch:

```toml
lsc-server = { git = "https://github.com/basmilius/language-server-core", tag = "v0.1.0" }
```

## What belongs here

Only what is demonstrably the same in more than one server, or in one server and certainly in the next. A language's kinds, grammar, settings, conversions of its own results to LSP and its capabilities stay in that server. Nothing here names a language. When a server needs something new from the core, it comes here first, with tests, and the server moves its pin.

A change must not cost a server performance: the parser is generic so it is monomorphized in the server's crate, and the PHP server's benchmarks (`cargo bench -p php-syntax`, `MEASUREMENTS.md` there) are the yardstick for a change to `lsc-syntax` or `lsc-text`.

## Checks

All of these pass before a commit; CI (`.github/workflows/ci.yml`) runs them on every push to main and every PR.

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

A change that a server will use is also checked in that server against the local checkout (below), with the server's own checks.

## Working on a server against a local checkout

A server keeps its Git pin committed. To build it against `../core` instead, put a `[patch]` in its `.cargo/config.toml`, which every server's `.gitignore` keeps out of Git:

```toml
[patch."https://github.com/basmilius/language-server-core"]
lsc-server = { path = "../core/crates/server" }
lsc-syntax = { path = "../core/crates/syntax" }
lsc-text = { path = "../core/crates/text" }
```

The paths are relative to the server's folder. While the patch is there, Cargo rewrites the core's entries in the server's `Cargo.lock` to the local paths, so run the checks without `--locked` and do not commit `Cargo.lock`. To go back, remove the file: the next Cargo command without `--locked` puts the pinned tag back in the lock, and `git diff Cargo.lock` is empty again. For a single command, the same patch fits on the command line:

```sh
cargo --config 'patch."https://github.com/basmilius/language-server-core".lsc-text.path="../core/crates/text"' test
```

## Releases

- The version lives in `[workspace.package]` of `Cargo.toml`; the tag is `v<version>`. A tag is never moved or deleted: a fix is a new version.
- Tag only a commit whose CI is green, push the tag, then move the servers' pins to it, each with its own checks.
- Push and tag only when Bas asks.

## Conventions

- Rust 2024, `rustfmt.toml`, `unsafe` forbidden, `rust-version` 1.85, so no let chains. `.editorconfig` is the rule for the rest.
- Every public item has tests here, not only through a server. Tests are deterministic: no sleeps, no wall clock.
- American English everywhere. Never an em dash or an en dash.
- Comments say why, never what the code already says.
- Conventional commits in English. No attribution lines.
- Never name another product in code, comments, docs or commits.
