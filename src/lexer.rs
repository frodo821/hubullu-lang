//! Hand-written lexer for the LexDSL language.
//!
//! Tokenises source text into [`Token`]s. Notable rules:
//! - `#` after whitespace (or at line start) starts a comment; after non-whitespace it is a `Hash` token
//! - `_` followed by XID\_Continue is an identifier; standalone `_` is an `Underscore` token (wildcard)
//! - Digits are valid identifier starts (e.g. `person=1`)

use crate::ast::{Span, Spanned};
use crate::error::Diagnostic;
use crate::span::FileId;
use crate::token::{TemplateSeg, Token, TokenKind};

/// Hand-written scanner for LexDSL source text.
pub struct Lexer<'a> {
    source: &'a str,
    file_id: FileId,
    pos: usize,
    errors: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    /// Create a new lexer for the given source text.
    pub fn new(source: &'a str, file_id: FileId) -> Self {
        Self {
            source,
            file_id,
            pos: 0,
            errors: Vec::new(),
        }
    }

    /// Consume the lexer and return all tokens plus any diagnostics.
    pub fn tokenize(mut self) -> (Vec<Token>, Vec<Diagnostic>) {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            if self.pos >= self.source.len() {
                tokens.push(self.make_token(TokenKind::Eof, self.pos, self.pos));
                break;
            }
            match self.next_token() {
                Some(tok) => tokens.push(tok),
                None => {
                    // Skip unknown byte and continue
                    let start = self.pos;
                    self.pos += 1;
                    self.errors.push(
                        Diagnostic::error("unexpected character").with_label(
                            self.span(start, self.pos),
                            "here",
                        ),
                    );
                }
            }
        }
        (tokens, self.errors)
    }

    fn skip_whitespace_and_comments(&mut self) {
        while self.pos < self.source.len() {
            let b = self.source.as_bytes()[self.pos];
            if b == b'#' {
                // # is a comment only if preceded by whitespace or at start of input.
                // After a non-whitespace char (e.g. `faren#motion`), it's a Hash token.
                if self.pos == 0
                    || self.source.as_bytes()[self.pos - 1].is_ascii_whitespace()
                {
                    while self.pos < self.source.len()
                        && self.source.as_bytes()[self.pos] != b'\n'
                    {
                        self.pos += 1;
                    }
                } else {
                    break;
                }
            } else if b.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.pos..].chars().next()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.source[self.pos + offset..].chars().next()
    }

    fn advance(&mut self) -> char {
        let ch = self.source[self.pos..].chars().next().unwrap();
        self.pos += ch.len_utf8();
        ch
    }

    fn span(&self, start: usize, end: usize) -> Span {
        Span {
            file_id: self.file_id,
            start,
            end,
        }
    }

    fn make_token(&self, kind: TokenKind, start: usize, end: usize) -> Token {
        Spanned {
            node: kind,
            span: self.span(start, end),
        }
    }

    fn next_token(&mut self) -> Option<Token> {
        let start = self.pos;
        let ch = self.peek()?;

        match ch {
            '{' => {
                self.advance();
                Some(self.make_token(TokenKind::LBrace, start, self.pos))
            }
            '}' => {
                self.advance();
                Some(self.make_token(TokenKind::RBrace, start, self.pos))
            }
            '[' => {
                self.advance();
                Some(self.make_token(TokenKind::LBracket, start, self.pos))
            }
            ']' => {
                self.advance();
                Some(self.make_token(TokenKind::RBracket, start, self.pos))
            }
            '(' => {
                self.advance();
                Some(self.make_token(TokenKind::LParen, start, self.pos))
            }
            ')' => {
                self.advance();
                Some(self.make_token(TokenKind::RParen, start, self.pos))
            }
            ':' => {
                self.advance();
                Some(self.make_token(TokenKind::Colon, start, self.pos))
            }
            ',' => {
                self.advance();
                Some(self.make_token(TokenKind::Comma, start, self.pos))
            }
            '.' => {
                self.advance();
                Some(self.make_token(TokenKind::Dot, start, self.pos))
            }
            '=' => {
                self.advance();
                Some(self.make_token(TokenKind::Eq, start, self.pos))
            }
            '+' => {
                self.advance();
                Some(self.make_token(TokenKind::Plus, start, self.pos))
            }
            '*' => {
                self.advance();
                Some(self.make_token(TokenKind::Star, start, self.pos))
            }
            '/' => {
                self.advance();
                Some(self.make_token(TokenKind::Slash, start, self.pos))
            }
            '!' => {
                self.advance();
                Some(self.make_token(TokenKind::Bang, start, self.pos))
            }
            '|' => {
                self.advance();
                Some(self.make_token(TokenKind::Pipe, start, self.pos))
            }
            '-' => {
                if self.peek_at(1) == Some('>') {
                    self.advance();
                    self.advance();
                    Some(self.make_token(TokenKind::Arrow, start, self.pos))
                } else {
                    None
                }
            }
            '#' => {
                self.advance();
                Some(self.make_token(TokenKind::Hash, start, self.pos))
            }
            '"' => Some(self.lex_string()),
            '`' => Some(self.lex_template()),
            '@' => self.lex_at_directive(),
            '_' => {
                // Check if next char is XID_Continue → identifier
                let next_pos = self.pos + 1;
                if next_pos < self.source.len() {
                    let next_ch = self.source[next_pos..].chars().next().unwrap();
                    if unicode_xid::UnicodeXID::is_xid_continue(next_ch) {
                        return Some(self.lex_ident());
                    }
                }
                self.advance();
                Some(self.make_token(TokenKind::Underscore, start, self.pos))
            }
            _ if unicode_xid::UnicodeXID::is_xid_start(ch) || ch == '_' => {
                Some(self.lex_ident())
            }
            _ if ch.is_ascii_digit() => {
                // Allow digit-started identifiers for tag values like person=1
                Some(self.lex_digit_ident())
            }
            _ => None,
        }
    }

    fn lex_ident(&mut self) -> Token {
        let start = self.pos;
        let ch = self.advance();
        debug_assert!(unicode_xid::UnicodeXID::is_xid_start(ch) || ch == '_');
        while self.pos < self.source.len() {
            let ch = self.source[self.pos..].chars().next().unwrap();
            if unicode_xid::UnicodeXID::is_xid_continue(ch) {
                self.pos += ch.len_utf8();
            } else {
                break;
            }
        }
        let text = self.source[start..self.pos].to_string();
        self.make_token(TokenKind::Ident(text), start, self.pos)
    }

    fn lex_digit_ident(&mut self) -> Token {
        let start = self.pos;
        while self.pos < self.source.len() {
            let ch = self.source[self.pos..].chars().next().unwrap();
            if unicode_xid::UnicodeXID::is_xid_continue(ch) || ch.is_ascii_digit() {
                self.pos += ch.len_utf8();
            } else {
                break;
            }
        }
        let text = self.source[start..self.pos].to_string();
        self.make_token(TokenKind::Ident(text), start, self.pos)
    }

    fn lex_string(&mut self) -> Token {
        let start = self.pos;
        self.advance(); // skip opening "
        let mut value = String::new();
        loop {
            if self.pos >= self.source.len() {
                self.errors.push(
                    Diagnostic::error("unterminated string literal")
                        .with_label(self.span(start, self.pos), "string starts here"),
                );
                break;
            }
            let ch = self.advance();
            match ch {
                '"' => break,
                '\\' => {
                    if self.pos < self.source.len() {
                        let esc = self.advance();
                        match esc {
                            'n' => value.push('\n'),
                            't' => value.push('\t'),
                            '\\' => value.push('\\'),
                            '"' => value.push('"'),
                            _ => {
                                self.errors.push(
                                    Diagnostic::error(format!("unknown escape sequence '\\{}'", esc))
                                        .with_label(
                                            self.span(self.pos - 2, self.pos),
                                            "unknown escape",
                                        ),
                                );
                                value.push(esc);
                            }
                        }
                    }
                }
                _ => value.push(ch),
            }
        }
        self.make_token(TokenKind::StringLit(value), start, self.pos)
    }

    fn lex_template(&mut self) -> Token {
        let start = self.pos;
        self.advance(); // skip opening `
        let mut segments = Vec::new();
        let mut current_lit = String::new();

        loop {
            if self.pos >= self.source.len() {
                self.errors.push(
                    Diagnostic::error("unterminated template literal")
                        .with_label(self.span(start, self.pos), "template starts here"),
                );
                break;
            }
            let ch = self.peek().unwrap();
            match ch {
                '`' => {
                    self.advance();
                    break;
                }
                '\\' => {
                    self.advance();
                    if self.pos < self.source.len() {
                        let esc = self.advance();
                        match esc {
                            '{' => current_lit.push('{'),
                            '}' => current_lit.push('}'),
                            '`' => current_lit.push('`'),
                            '\\' => current_lit.push('\\'),
                            _ => {
                                self.errors.push(
                                    Diagnostic::error(format!(
                                        "unknown escape sequence '\\{}' in template",
                                        esc
                                    ))
                                    .with_label(
                                        self.span(self.pos - 2, self.pos),
                                        "unknown escape",
                                    ),
                                );
                                current_lit.push(esc);
                            }
                        }
                    }
                }
                '{' => {
                    self.advance();
                    if !current_lit.is_empty() {
                        segments.push(TemplateSeg::Lit(std::mem::take(&mut current_lit)));
                    }
                    // Parse interpolation: {name} or {stem.slot}
                    self.skip_ws_inline();
                    let name = self.read_ident_inline();
                    self.skip_ws_inline();
                    if self.peek() == Some('.') {
                        self.advance();
                        self.skip_ws_inline();
                        let slot = self.read_ident_inline();
                        self.skip_ws_inline();
                        segments.push(TemplateSeg::SlotInterp {
                            stem: name,
                            slot,
                        });
                    } else {
                        segments.push(TemplateSeg::Interp(name));
                    }
                    if self.peek() == Some('}') {
                        self.advance();
                    } else {
                        self.errors.push(
                            Diagnostic::error("expected '}' in template interpolation")
                                .with_label(self.span(self.pos, self.pos + 1), "expected '}'"),
                        );
                    }
                }
                _ => {
                    current_lit.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
        if !current_lit.is_empty() {
            segments.push(TemplateSeg::Lit(current_lit));
        }
        self.make_token(TokenKind::TemplateLit(segments), start, self.pos)
    }

    fn skip_ws_inline(&mut self) {
        while self.pos < self.source.len() {
            let b = self.source.as_bytes()[self.pos];
            if b == b' ' || b == b'\t' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn read_ident_inline(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.source.len() {
            let ch = self.source[self.pos..].chars().next().unwrap();
            if unicode_xid::UnicodeXID::is_xid_continue(ch) || (self.pos == start && ch == '_') {
                self.pos += ch.len_utf8();
            } else {
                break;
            }
        }
        self.source[start..self.pos].to_string()
    }

    fn lex_at_directive(&mut self) -> Option<Token> {
        let start = self.pos;
        self.advance(); // skip @
        let ident_start = self.pos;
        while self.pos < self.source.len() {
            let ch = self.source[self.pos..].chars().next().unwrap();
            if ch.is_ascii_alphanumeric() || ch == '_' {
                self.pos += ch.len_utf8();
            } else {
                break;
            }
        }
        let directive = &self.source[ident_start..self.pos];
        let kind = match directive {
            "use" => TokenKind::AtUse,
            "reference" => TokenKind::AtReference,
            "extend" => TokenKind::AtExtend,
            "render" => TokenKind::AtRender,
            _ => {
                self.errors.push(
                    Diagnostic::error(format!("unknown directive '@{}'", directive))
                        .with_label(self.span(start, self.pos), "unknown directive"),
                );
                return None;
            }
        };
        Some(self.make_token(kind, start, self.pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(input: &str) -> Vec<TokenKind> {
        let lexer = Lexer::new(input, FileId(0));
        let (tokens, errors) = lexer.tokenize();
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        tokens.into_iter().map(|t| t.node).collect()
    }

    #[test]
    fn test_punctuation() {
        let tokens = lex("{ } [ ] ( ) : , . = + ->");
        assert_eq!(
            tokens,
            vec![
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Colon,
                TokenKind::Comma,
                TokenKind::Dot,
                TokenKind::Eq,
                TokenKind::Plus,
                TokenKind::Arrow,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_ident() {
        let tokens = lex("foo bar_baz tense123");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Ident("foo".into()),
                TokenKind::Ident("bar_baz".into()),
                TokenKind::Ident("tense123".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_underscore_vs_ident() {
        let tokens = lex("_ _foo");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Underscore,
                TokenKind::Ident("_foo".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_string_lit() {
        let tokens = lex(r#""hello" "esc\"ape""#);
        assert_eq!(
            tokens,
            vec![
                TokenKind::StringLit("hello".into()),
                TokenKind::StringLit("esc\"ape".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_template_simple() {
        let tokens = lex("`{pres}e`");
        assert_eq!(
            tokens,
            vec![
                TokenKind::TemplateLit(vec![
                    TemplateSeg::Interp("pres".into()),
                    TemplateSeg::Lit("e".into()),
                ]),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_template_slot() {
        let tokens = lex("`{root1.C1}a{root1.C2}`");
        assert_eq!(
            tokens,
            vec![
                TokenKind::TemplateLit(vec![
                    TemplateSeg::SlotInterp {
                        stem: "root1".into(),
                        slot: "C1".into(),
                    },
                    TemplateSeg::Lit("a".into()),
                    TemplateSeg::SlotInterp {
                        stem: "root1".into(),
                        slot: "C2".into(),
                    },
                ]),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_empty_template() {
        let tokens = lex("``");
        assert_eq!(
            tokens,
            vec![TokenKind::TemplateLit(vec![]), TokenKind::Eof]
        );
    }

    #[test]
    fn test_at_directives() {
        let tokens = lex("@use @reference @extend");
        assert_eq!(
            tokens,
            vec![
                TokenKind::AtUse,
                TokenKind::AtReference,
                TokenKind::AtExtend,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_comment() {
        let tokens = lex("foo # this is a comment\nbar");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Ident("foo".into()),
                TokenKind::Ident("bar".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_unicode_ident() {
        let tokens = lex("品詞 たべる");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Ident("品詞".into()),
                TokenKind::Ident("たべる".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_hash_meaning_ref() {
        let tokens = lex("faren#motion");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Ident("faren".into()),
                TokenKind::Hash,
                TokenKind::Ident("motion".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_hash_comment_vs_token() {
        // # after whitespace = comment
        let tokens = lex("foo # comment\nbar");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Ident("foo".into()),
                TokenKind::Ident("bar".into()),
                TokenKind::Eof,
            ]
        );
    }
}
