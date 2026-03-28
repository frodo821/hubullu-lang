//! Token types produced by the LexDSL lexer.

use crate::ast::{Span, Spanned};

/// The kind of a lexical token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    // Identifiers & literals
    Ident(String),
    StringLit(String),
    /// Template literal segments (backtick-delimited).
    TemplateLit(Vec<TemplateSeg>),

    // Punctuation
    LBrace,    // {
    RBrace,    // }
    LBracket,  // [
    RBracket,  // ]
    LParen,    // (
    RParen,    // )
    Colon,     // :
    Comma,     // ,
    Dot,       // .
    Hash,      // #
    Eq,        // =
    Arrow,     // ->
    Plus,      // +
    Underscore, // _ (standalone wildcard)
    Star,       // *

    // @-prefixed directives
    AtUse,       // @use
    AtReference, // @reference
    AtExtend,    // @extend

    // End of file
    Eof,
}

/// A segment of a template literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateSeg {
    /// Literal text between interpolations.
    Lit(String),
    /// `{name}` — stem interpolation.
    Interp(String),
    /// `{stem.slot}` — structural slot interpolation.
    SlotInterp { stem: String, slot: String },
}

/// A token with source span information.
pub type Token = Spanned<TokenKind>;

impl Token {
    /// Create a dummy EOF token at position 0 (for parser initialization).
    pub fn dummy_eof() -> Self {
        Spanned {
            node: TokenKind::Eof,
            span: Span {
                file_id: crate::span::FileId(0),
                start: 0,
                end: 0,
            },
        }
    }
}
