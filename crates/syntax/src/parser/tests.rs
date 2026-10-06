//! A small language to drive the parser with: statements `let name = expr;` where an expression
//! is a number, a name, a sum or a parenthesized expression, with `//` comments and `///` doc
//! comments.

use rowan::{Language, NodeOrToken, TextRange, TextSize};

use super::*;
use Kind::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
enum Kind {
    WHITESPACE,
    COMMENT,
    DOC_COMMENT,
    IDENT,
    NUMBER,
    LET_KW,
    PLUS,
    EQ,
    SEMICOLON,
    LPAREN,
    RPAREN,
    UNKNOWN,
    EOF,
    ERROR,
    ROOT,
    LET,
    BINARY,
    PAREN,
    LITERAL,
}

const KINDS: [Kind; 19] = [
    WHITESPACE,
    COMMENT,
    DOC_COMMENT,
    IDENT,
    NUMBER,
    LET_KW,
    PLUS,
    EQ,
    SEMICOLON,
    LPAREN,
    RPAREN,
    UNKNOWN,
    EOF,
    ERROR,
    ROOT,
    LET,
    BINARY,
    PAREN,
    LITERAL,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Lang {}

impl Language for Lang {
    type Kind = Kind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> Kind {
        KINDS[raw.0 as usize]
    }

    fn kind_to_raw(kind: Kind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

impl TokenKind for Kind {
    const EOF: Kind = EOF;
    const ERROR: Kind = ERROR;

    fn is_trivia(self) -> bool {
        matches!(self, WHITESPACE | COMMENT | DOC_COMMENT)
    }

    fn is_whitespace(self) -> bool {
        self == WHITESPACE
    }

    fn is_doc_comment(self) -> bool {
        self == DOC_COMMENT
    }
}

type P<'a> = Parser<'a, Lang>;

fn lex(text: &str) -> Vec<Token<Kind>> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let rest = &text[at..];
        let (kind, len) = if rest.starts_with("//") {
            let len = rest.find('\n').unwrap_or(rest.len());
            let kind = if rest.starts_with("///") { DOC_COMMENT } else { COMMENT };
            (kind, len)
        } else if bytes[at].is_ascii_whitespace() {
            let len = rest.find(|c: char| !c.is_ascii_whitespace()).unwrap_or(rest.len());
            (WHITESPACE, len)
        } else if bytes[at].is_ascii_alphabetic() {
            let len = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len());
            let kind = if &rest[..len] == "let" { LET_KW } else { IDENT };
            (kind, len)
        } else if bytes[at].is_ascii_digit() {
            let len = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
            (NUMBER, len)
        } else {
            let character = rest.chars().next().expect("not at the end");
            let kind = match character {
                '+' => PLUS,
                '=' => EQ,
                ';' => SEMICOLON,
                '(' => LPAREN,
                ')' => RPAREN,
                _ => UNKNOWN,
            };
            (kind, character.len_utf8())
        };
        tokens.push(Token { kind, len: len as u32 });
        at += len;
    }
    tokens
}

fn parse(text: &str) -> Parse<Lang> {
    let mut p = P::new(text, lex(text));
    p.start_root(ROOT);
    while !p.eof() {
        if p.at(LET_KW) {
            statement(&mut p);
        } else {
            p.error_recover(&[LET_KW]);
        }
    }
    p.flush_rest();
    p.finish_node();
    p.finish()
}

fn statement(p: &mut P) {
    p.start_before_declaration(LET);
    p.bump();
    p.expect(IDENT, "Name");
    p.expect(EQ, "'='");
    expr(p);
    p.expect(SEMICOLON, "';'");
    p.finish_node();
}

fn expr(p: &mut P) {
    let checkpoint = p.checkpoint();
    if !operand(p) {
        return;
    }
    while p.at(PLUS) {
        p.start_at(checkpoint, BINARY);
        p.bump();
        operand(p);
        p.finish_node();
    }
}

fn operand(p: &mut P) -> bool {
    match p.current() {
        NUMBER | IDENT => {
            p.start(LITERAL);
            p.bump();
            p.finish_node();
            true
        }
        LPAREN => {
            if !p.enter() {
                p.error_bump();
                return false;
            }
            p.start(PAREN);
            p.bump();
            expr(p);
            p.expect(RPAREN, "')'");
            p.finish_node();
            p.leave();
            true
        }
        _ => {
            p.error_expected("Expression");
            false
        }
    }
}

/// The tree as `(KIND child ...)`, tokens as their quoted text, trivia included.
fn render(parse: &Parse<Lang>) -> String {
    fn walk(element: rowan::SyntaxElement<Lang>, out: &mut String) {
        match element {
            NodeOrToken::Node(node) => {
                out.push_str(&format!("({:?}", node.kind()));
                for child in node.children_with_tokens() {
                    out.push(' ');
                    walk(child, out);
                }
                out.push(')');
            }
            NodeOrToken::Token(token) => out.push_str(&format!("{:?}", token.text())),
        }
    }
    let mut out = String::new();
    walk(NodeOrToken::Node(parse.syntax()), &mut out);
    out
}

fn errors(parse: &Parse<Lang>) -> Vec<(u32, u32, &str)> {
    parse
        .errors()
        .iter()
        .map(|error| {
            (
                u32::from(error.range.start()),
                u32::from(error.range.end()),
                error.message.as_str(),
            )
        })
        .collect()
}

#[test]
fn builds_a_tree_with_trivia_between_nodes() {
    let parse = parse("  let a = 1 + (b) ;");
    assert_eq!(
        render(&parse),
        r#"(ROOT "  " (LET "let" " " "a" " " "=" " " (BINARY (LITERAL "1") " " "+" " " (PAREN "(" (LITERAL "b") ")")) " " ";"))"#
    );
    assert!(parse.errors().is_empty());
}

#[test]
fn every_byte_of_the_input_is_in_the_tree() {
    let samples = [
        "",
        "   ",
        "let a = 1;",
        "let = ;",
        "garbage ) ( let x = (((1;",
        "/// doc\n// plain\nlet a = 2 +;\n\u{1f600} let",
        "let a = 1 + + 2;\r\n;;",
    ];
    for sample in samples {
        let text = sample.to_string();
        for end in (0..=text.len()).filter(|end| text.is_char_boundary(*end)) {
            let prefix = &text[..end];
            assert_eq!(parse(prefix).syntax().to_string(), prefix);
        }
    }
}

#[test]
fn a_declaration_takes_the_doc_comment_before_it() {
    let parse = parse("/// one\n\nlet a = 1;");
    assert_eq!(
        render(&parse),
        r#"(ROOT (LET "/// one" "\n\n" "let" " " "a" " " "=" " " (LITERAL "1") ";"))"#
    );
}

#[test]
fn a_plain_comment_stays_outside_the_declaration() {
    let parse = parse("/// doc\n// plain\nlet a = 1;");
    assert_eq!(
        render(&parse),
        r#"(ROOT "/// doc" "\n" "// plain" "\n" (LET "let" " " "a" " " "=" " " (LITERAL "1") ";"))"#
    );
}

#[test]
fn a_missing_token_is_reported_after_the_previous_one_and_not_consumed() {
    let parse = parse("let a  1;");
    assert_eq!(errors(&parse), [(5, 5, "'=' expected")]);
    assert_eq!(render(&parse), r#"(ROOT (LET "let" " " "a" "  " (LITERAL "1") ";"))"#);
}

#[test]
fn a_missing_token_at_the_start_is_reported_before_the_first_token() {
    let mut p = P::new("  x", lex("  x"));
    p.start_root(ROOT);
    p.error_expected("Name");
    p.flush_rest();
    p.finish_node();
    assert_eq!(errors(&p.finish()), [(2, 2, "Name expected")]);
}

#[test]
fn recovers_by_wrapping_what_it_cannot_read_in_error_nodes() {
    let parse = parse("1 + ) let a = 1;");
    assert_eq!(errors(&parse), [(0, 1, "Unexpected '1'")]);
    assert_eq!(
        render(&parse),
        r#"(ROOT (ERROR "1" " " "+" " " ")") " " (LET "let" " " "a" " " "=" " " (LITERAL "1") ";"))"#
    );
}

#[test]
fn error_bump_wraps_one_token_and_escapes_it_in_the_message() {
    let text = "\"\n";
    let mut p = P::new(text, lex(text));
    p.start_root(ROOT);
    p.error_bump();
    p.error_bump();
    p.flush_rest();
    p.finish_node();
    let parse = p.finish();
    assert_eq!(errors(&parse), [(0, 1, "Unexpected '\\\"'")]);
    assert_eq!(render(&parse), r#"(ROOT (ERROR "\"") "\n")"#);
}

#[test]
fn deep_nesting_ends_in_one_error_and_not_in_a_stack_overflow() {
    let depth = MAX_DEPTH as usize + 50;
    let text = format!("let a = {}1{};", "(".repeat(depth), ")".repeat(depth));
    let parse = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            let parse = parse(&text);
            assert_eq!(parse.syntax().to_string(), text);
            parse
        })
        .expect("spawned")
        .join()
        .expect("no panic");
    let too_deep = parse
        .errors()
        .iter()
        .filter(|error| error.message == "Nesting is too deep")
        .count();
    assert_eq!(too_deep, 1);
}

#[test]
fn leaving_more_than_entering_does_not_underflow() {
    let mut p = P::new("", Vec::new());
    p.leave();
    assert!(p.enter());
}

#[test]
fn runs_out_of_fuel_when_nothing_is_consumed() {
    let text = "a";
    let p = P::new(text, lex(text));
    for _ in 0..FUEL {
        assert_eq!(p.current(), IDENT);
    }
    assert_eq!(p.current(), EOF);
    assert!(p.eof());
}

#[test]
fn bumping_a_token_refills_the_fuel() {
    let text = "a b";
    let mut p = P::new(text, lex(text));
    p.start_root(ROOT);
    for _ in 0..FUEL {
        p.current();
    }
    p.bump();
    assert_eq!(p.current(), IDENT);
}

#[test]
fn looks_ahead_over_trivia() {
    let text = "let /* */ a=1 // c\n;";
    let p = P::new(text, lex(text));
    assert_eq!(p.nth(0), LET_KW);
    assert_eq!(p.nth(1), UNKNOWN);
    assert_eq!(p.nth_text(1), "/");
    let text = "let  a=1 // c\n;";
    let p = P::new(text, lex(text));
    let kinds: Vec<Kind> = (0..6).map(|n| p.nth(n)).collect();
    assert_eq!(kinds, [LET_KW, IDENT, EQ, NUMBER, SEMICOLON, EOF]);
    assert_eq!(p.nth_text(1), "a");
    assert_eq!(p.nth_text(9), "");
    assert!(p.at(LET_KW));
    assert!(p.at_any(&[IDENT, LET_KW]));
    assert!(!p.at_any(&[IDENT]));
}

#[test]
fn tells_whether_tokens_touch() {
    let text = "a=1 ;";
    let mut p = P::new(text, lex(text));
    p.start_root(ROOT);
    assert!(p.nth_touches_next(0));
    assert!(p.nth_touches_next(1));
    assert!(!p.nth_touches_next(2));
    assert!(!p.nth_touches_next(3));
    assert!(!p.nth_touches_prev());
    p.bump();
    assert!(p.nth_touches_prev());
    p.bump();
    p.bump();
    assert!(!p.nth_touches_prev());
}

#[test]
fn knows_where_the_cursor_is() {
    let text = "ab  cd";
    let mut p = P::new(text, lex(text));
    p.start_root(ROOT);
    assert_eq!(p.position(), 0);
    assert_eq!(p.current_offset(), 0);
    assert_eq!(p.current_range(), TextRange::new(TextSize::from(0), TextSize::from(2)));
    p.bump();
    assert_eq!(p.position(), 1);
    assert_eq!(p.current_offset(), 4);
    assert_eq!(p.byte_before(4), Some(b' '));
    assert_eq!(p.byte_before(0), None);
    assert_eq!(p.after_previous_range(), TextRange::empty(TextSize::from(2)));
    p.bump();
    p.bump();
    assert_eq!(p.position(), 2);
    assert_eq!(p.current_offset(), 6);
    assert_eq!(p.current_range(), TextRange::empty(TextSize::from(6)));
    assert_eq!(p.text(), text);
}

#[test]
fn a_declaration_checkpoint_takes_the_doc_comment_into_the_node_started_at_it() {
    let text = "/// doc\n  a";
    let mut p = P::new(text, lex(text));
    p.start_root(ROOT);
    let checkpoint = p.declaration_checkpoint();
    p.bump();
    p.start_at(checkpoint, LITERAL);
    p.finish_node();
    p.flush_rest();
    p.finish_node();
    assert_eq!(render(&p.finish()), r#"(ROOT (LITERAL "/// doc" "\n  " "a"))"#);
}

#[test]
fn eat_consumes_only_the_kind_it_is_given() {
    let text = "a;";
    let mut p = P::new(text, lex(text));
    p.start_root(ROOT);
    assert!(!p.eat(SEMICOLON));
    assert!(p.eat(IDENT));
    assert!(p.expect(SEMICOLON, "';'"));
    assert!(!p.expect(SEMICOLON, "';'"));
    p.finish_node();
    let parse = p.finish();
    assert_eq!(errors(&parse), [(2, 2, "';' expected")]);
}

#[test]
fn errors_can_be_added_with_a_range_of_their_own() {
    let mut p = P::new("", Vec::new());
    p.start_root(ROOT);
    p.error_at(TextRange::new(TextSize::from(0), TextSize::from(0)), "Lexer trouble");
    p.error_here("Here");
    p.finish_node();
    assert_eq!(errors(&p.finish()), [(0, 0, "Lexer trouble"), (0, 0, "Here")]);
}

#[test]
fn a_parse_clones_and_prints() {
    let parse = parse("let a = 1");
    let copy = parse.clone();
    assert_eq!(copy.green(), parse.green());
    assert_eq!(copy.errors(), parse.errors());
    assert!(format!("{parse:?}").contains("';' expected"));
}
