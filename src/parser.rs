//! Recursive descent parser for the Hubullu language.
//!
//! Converts a token stream into an [`ast::File`]. Recovers from errors at
//! the top-level item boundary so that multiple diagnostics can be reported
//! in a single pass.

use crate::ast::*;
use crate::error::Diagnostic;
use crate::span::FileId;
use crate::token::{TemplateSeg, Token, TokenKind};

/// Recursive descent parser for Hubullu.
pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    file_id: FileId,
    errors: Vec<Diagnostic>,
}

impl Parser {
    /// Create a new parser from a token stream.
    pub fn new(tokens: Vec<Token>, file_id: FileId) -> Self {
        Self {
            tokens,
            pos: 0,
            file_id,
            errors: Vec::new(),
        }
    }

    /// Consume the parser and return the AST plus any diagnostics.
    pub fn parse(mut self) -> (File, Vec<Diagnostic>) {
        let mut items = Vec::with_capacity(16);
        while !self.at_eof() {
            match self.parse_item() {
                Ok(item) => items.push(item),
                Err(diag) => {
                    self.errors.push(diag);
                    self.recover_to_top_level();
                }
            }
        }
        (File { items }, self.errors)
    }

    // -----------------------------------------------------------------------
    // Token helpers
    // -----------------------------------------------------------------------

    fn peek(&self) -> &TokenKind {
        self.tokens.get(self.pos).map(|t| &t.node).unwrap_or(&TokenKind::Eof)
    }

    fn peek_token(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&self.tokens[self.tokens.len() - 1])
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn at(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.peek()) == std::mem::discriminant(kind)
    }

    fn at_ident(&self, name: &str) -> bool {
        matches!(self.peek(), TokenKind::Ident(s) if s == name)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<Token, Diagnostic> {
        if self.at(kind) {
            Ok(self.advance())
        } else {
            Err(self.error(format!("expected {:?}, found {:?}", kind, self.peek())))
        }
    }

    fn expect_ident(&mut self) -> Result<Ident, Diagnostic> {
        if let TokenKind::Ident(_) = self.peek() {
            let tok = self.advance();
            if let TokenKind::Ident(s) = tok.node {
                Ok(Spanned::new(s, tok.span))
            } else {
                unreachable!()
            }
        } else {
            Err(self.error(format!("expected identifier, found {:?}", self.peek())))
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<Token, Diagnostic> {
        if self.at_ident(kw) {
            Ok(self.advance())
        } else {
            Err(self.error(format!("expected '{}', found {:?}", kw, self.peek())))
        }
    }

    fn expect_string(&mut self) -> Result<StringLit, Diagnostic> {
        if let TokenKind::StringLit(_) = self.peek() {
            let tok = self.advance();
            if let TokenKind::StringLit(s) = tok.node {
                Ok(Spanned::new(s, tok.span))
            } else {
                unreachable!()
            }
        } else {
            Err(self.error(format!("expected string literal, found {:?}", self.peek())))
        }
    }

    fn span_from(&self, start: usize) -> Span {
        let end = if self.pos > 0 {
            self.tokens[self.pos - 1].span.end
        } else {
            start
        };
        Span {
            file_id: self.file_id,
            start,
            end,
        }
    }

    fn current_span(&self) -> Span {
        self.peek_token().span
    }

    fn error(&self, msg: impl Into<String>) -> Diagnostic {
        Diagnostic::error(msg).with_label(self.current_span(), "here")
    }

    fn recover_to_top_level(&mut self) {
        // Skip tokens until we find something that looks like a top-level start.
        loop {
            match self.peek() {
                TokenKind::Eof => break,
                TokenKind::AtUse | TokenKind::AtReference | TokenKind::AtExport | TokenKind::AtExtend | TokenKind::AtRender => break,
                TokenKind::Ident(s) if matches!(s.as_str(), "tagaxis" | "inflection" | "entry" | "phonrule") => {
                    break
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Top-level items
    // -----------------------------------------------------------------------

    fn parse_item(&mut self) -> Result<Spanned<Item>, Diagnostic> {
        let start = self.current_span().start;
        let item = match self.peek() {
            TokenKind::AtUse => {
                self.advance();
                Item::Use(self.parse_import()?)
            }
            TokenKind::AtReference => {
                self.advance();
                Item::Reference(self.parse_import()?)
            }
            TokenKind::AtExport => {
                self.advance();
                Item::Export(self.parse_export()?)
            }
            TokenKind::AtExtend => {
                self.advance();
                Item::Extend(self.parse_extend()?)
            }
            TokenKind::AtRender => {
                self.advance();
                Item::Render(self.parse_render_config()?)
            }
            TokenKind::Ident(s) if s == "tagaxis" => {
                self.advance();
                Item::TagAxis(self.parse_tagaxis()?)
            }
            TokenKind::Ident(s) if s == "inflection" => {
                self.advance();
                Item::Inflection(self.parse_inflection()?)
            }
            TokenKind::Ident(s) if s == "entry" => {
                self.advance();
                Item::Entry(self.parse_entry()?)
            }
            TokenKind::Ident(s) if s == "phonrule" => {
                self.advance();
                Item::PhonRule(self.parse_phonrule()?)
            }
            _ => {
                return Err(self.error(format!(
                    "expected top-level item, found {:?}",
                    self.peek()
                )));
            }
        };
        Ok(Spanned::new(item, self.span_from(start)))
    }

    // -----------------------------------------------------------------------
    // @use / @reference
    // -----------------------------------------------------------------------

    fn parse_import(&mut self) -> Result<Import, Diagnostic> {
        let target = self.parse_import_target()?;
        self.expect_keyword("from")?;
        let path = self.expect_string()?;
        Ok(Import { target, path })
    }

    fn parse_import_target(&mut self) -> Result<ImportTarget, Diagnostic> {
        if matches!(self.peek(), TokenKind::Star) {
            self.advance();
            let alias = if self.at_ident("as") {
                self.advance();
                Some(self.expect_ident()?)
            } else {
                None
            };
            return Ok(ImportTarget::Glob { alias });
        }

        if matches!(self.peek(), TokenKind::LParen) {
            // Parenthesized named list
            self.advance();
            let entries = self.parse_import_entries()?;
            self.expect(&TokenKind::RParen)?;
            Ok(ImportTarget::Named(entries))
        } else {
            // Could be bare named list: `tense, aspect as a from "..."`
            let entries = self.parse_import_entries()?;
            Ok(ImportTarget::Named(entries))
        }
    }

    fn parse_import_entries(&mut self) -> Result<Vec<ImportEntry>, Diagnostic> {
        let mut entries = Vec::new();
        loop {
            let name = self.expect_ident()?;
            let alias = if self.at_ident("as") {
                self.advance();
                Some(self.expect_ident()?)
            } else {
                None
            };
            entries.push(ImportEntry { name, alias });
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
                // Allow trailing comma before ) or 'from'
                if matches!(self.peek(), TokenKind::RParen) || self.at_ident("from") {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(entries)
    }

    // -----------------------------------------------------------------------
    // @export
    // -----------------------------------------------------------------------

    fn parse_export(&mut self) -> Result<Export, Diagnostic> {
        let is_use = if self.at_ident("use") {
            self.advance();
            true
        } else if self.at_ident("reference") {
            self.advance();
            false
        } else {
            return Err(self.error("expected 'use' or 'reference' after @export"));
        };

        let target = self.parse_import_target()?;

        let path = if self.at_ident("from") {
            self.advance();
            Some(self.expect_string()?)
        } else {
            None
        };

        Ok(Export { is_use, target, path })
    }

    // -----------------------------------------------------------------------
    // tagaxis
    // -----------------------------------------------------------------------

    fn parse_tagaxis(&mut self) -> Result<TagAxis, Diagnostic> {
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut role = None;
        let mut display = None;
        let mut index = None;

        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            let field = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            match field.node.as_str() {
                "role" => {
                    let role_tok = self.expect_ident()?;
                    let r = match role_tok.node.as_str() {
                        "inflectional" => Role::Inflectional,
                        "classificatory" => Role::Classificatory,
                        "structural" => Role::Structural,
                        _ => {
                            return Err(Diagnostic::error(format!(
                                "unknown role '{}'",
                                role_tok.node
                            ))
                            .with_label(role_tok.span, "expected inflectional, classificatory, or structural"));
                        }
                    };
                    role = Some(Spanned::new(r, role_tok.span));
                }
                "display" => {
                    display = Some(self.parse_display_map()?);
                }
                "index" => {
                    let idx_tok = self.expect_ident()?;
                    let k = match idx_tok.node.as_str() {
                        "exact" => IndexKind::Exact,
                        "fulltext" => IndexKind::Fulltext,
                        _ => {
                            return Err(Diagnostic::error(format!(
                                "unknown index kind '{}'",
                                idx_tok.node
                            ))
                            .with_label(idx_tok.span, "expected exact or fulltext"));
                        }
                    };
                    index = Some(Spanned::new(k, idx_tok.span));
                }
                other => {
                    return Err(Diagnostic::error(format!(
                        "unknown tagaxis field '{}'",
                        other
                    ))
                    .with_label(field.span, "unknown field"));
                }
            }
        }

        self.expect(&TokenKind::RBrace)?;

        let role = role.ok_or_else(|| self.error("tagaxis missing 'role' field"))?;
        let display = display.unwrap_or_default();

        Ok(TagAxis {
            name,
            role,
            display,
            index,
        })
    }

    fn parse_display_map(&mut self) -> Result<DisplayMap, Diagnostic> {
        self.expect(&TokenKind::LBrace)?;
        let mut map = Vec::new();
        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            let key = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let value = self.expect_string()?;
            map.push((key, value));
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(map)
    }

    // -----------------------------------------------------------------------
    // @extend
    // -----------------------------------------------------------------------

    fn parse_extend(&mut self) -> Result<Extend, Diagnostic> {
        let name = self.expect_ident()?;
        self.expect_keyword("for")?;
        self.expect_keyword("tagaxis")?;
        let target_axis = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut values = Vec::new();
        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            values.push(self.parse_extend_value()?);
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(Extend {
            name,
            target_axis,
            values,
        })
    }

    fn parse_extend_value(&mut self) -> Result<ExtendValue, Diagnostic> {
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut display = Vec::new();
        let mut slots = Vec::new();

        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            if self.at_ident("display") {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                display = self.parse_display_map()?;
            } else if self.at_ident("slots") {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                self.expect(&TokenKind::LBracket)?;
                while !matches!(self.peek(), TokenKind::RBracket | TokenKind::Eof) {
                    slots.push(self.expect_ident()?);
                    if matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                    }
                }
                self.expect(&TokenKind::RBracket)?;
            } else {
                return Err(self.error(format!(
                    "unexpected field in @extend value: {:?}",
                    self.peek()
                )));
            }
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(ExtendValue {
            name,
            display,
            slots,
        })
    }

    // -----------------------------------------------------------------------
    // inflection
    // -----------------------------------------------------------------------

    fn parse_inflection(&mut self) -> Result<Inflection, Diagnostic> {
        let name = self.expect_ident()?;

        let display = if self.at_ident("display") {
            self.advance();
            self.parse_display_map()?
        } else {
            Vec::new()
        };

        self.expect_keyword("for")?;
        let axes = self.parse_axis_list()?;
        self.expect(&TokenKind::LBrace)?;

        let required_stems = if self.at_ident("requires") {
            self.parse_requires_stems()?
        } else {
            Vec::new()
        };

        let body = self.parse_inflection_body()?;
        self.expect(&TokenKind::RBrace)?;

        Ok(Inflection {
            name,
            display,
            axes,
            required_stems,
            body,
        })
    }

    fn parse_axis_list(&mut self) -> Result<Vec<Ident>, Diagnostic> {
        self.expect(&TokenKind::LBrace)?;
        let mut axes = Vec::new();
        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            axes.push(self.expect_ident()?);
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(axes)
    }

    fn parse_requires_stems(&mut self) -> Result<Vec<StemReq>, Diagnostic> {
        self.expect_keyword("requires")?;
        self.expect_keyword("stems")?;
        self.expect(&TokenKind::Colon)?;

        let mut stems = Vec::new();
        loop {
            let name = self.expect_ident()?;
            // Only parse [constraint] if it looks like a stem constraint,
            // not a rule start. A stem constraint is `[ident=ident, ...]`
            // followed by `,` or end-of-stems. A rule is `[...] -> ...`.
            // We disambiguate by checking: if `[` is followed by an ident and `=`,
            // AND the content doesn't contain `_`, it's a constraint.
            // Simpler heuristic: it's a constraint only if the bracket is
            // followed by `,` or another stem name after `]`.
            let constraint = if matches!(self.peek(), TokenKind::LBracket)
                && self.is_stem_constraint_ahead()
            {
                self.parse_tag_conditions()?
            } else {
                Vec::new()
            };
            stems.push(StemReq { name, constraint });
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
                match self.peek() {
                    TokenKind::Ident(_) => continue,
                    _ => break,
                }
            } else {
                break;
            }
        }
        Ok(stems)
    }

    /// Look ahead to determine if `[` starts a stem constraint or a rule.
    /// A stem constraint: `[ident=ident, ...]` followed by `,` or end.
    /// A rule: `[...] -> template/null/delegate`.
    fn is_stem_constraint_ahead(&self) -> bool {
        // Scan ahead from current pos to find the matching `]`,
        // then check if the token after `]` is `,` or an ident (another stem).
        // If it's `->`, it's a rule.
        let mut depth = 0;
        let mut i = self.pos;
        while i < self.tokens.len() {
            match &self.tokens[i].node {
                TokenKind::LBracket => depth += 1,
                TokenKind::RBracket => {
                    depth -= 1;
                    if depth == 0 {
                        // Check what follows ]
                        // A stem constraint is followed by `,` (more stems),
                        // an ident (next stem), `[` (first rule), `}` (end of block),
                        // `compose` keyword, or EOF.
                        let next = i + 1;
                        if next < self.tokens.len() {
                            return matches!(
                                &self.tokens[next].node,
                                TokenKind::Comma | TokenKind::Eof
                                    | TokenKind::LBracket | TokenKind::RBrace
                            ) || matches!(&self.tokens[next].node, TokenKind::Ident(_));
                        }
                        return true;
                    }
                }
                TokenKind::Arrow => {
                    // Seeing -> before closing ] means this is definitely a rule
                    return false;
                }
                TokenKind::Underscore if depth == 1 => {
                    // Wildcards don't appear in stem constraints
                    return false;
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    fn parse_tag_conditions(&mut self) -> Result<Vec<TagCondition>, Diagnostic> {
        self.expect(&TokenKind::LBracket)?;
        let mut conditions = Vec::new();
        while !matches!(
            self.peek(),
            TokenKind::RBracket | TokenKind::Eof
        ) {
            let axis = self.expect_ident()?;
            self.expect(&TokenKind::Eq)?;
            let value = self.expect_ident()?;
            conditions.push(TagCondition { axis, value });
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
        self.expect(&TokenKind::RBracket)?;
        Ok(conditions)
    }

    fn parse_tag_condition_list(&mut self) -> Result<TagConditionList, Diagnostic> {
        let start = self.current_span().start;
        self.expect(&TokenKind::LBracket)?;
        let mut conditions = Vec::new();
        let mut wildcard = false;

        while !matches!(self.peek(), TokenKind::RBracket | TokenKind::Eof) {
            if matches!(self.peek(), TokenKind::Underscore) {
                self.advance();
                wildcard = true;
                // _ must be last
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
                break;
            }
            let axis = self.expect_ident()?;
            self.expect(&TokenKind::Eq)?;
            let value = self.expect_ident()?;
            conditions.push(TagCondition { axis, value });
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }

        self.expect(&TokenKind::RBracket)?;
        Ok(TagConditionList {
            conditions,
            wildcard,
            span: self.span_from(start),
        })
    }

    fn parse_inflection_body(&mut self) -> Result<InflectionBody, Diagnostic> {
        if self.at_ident("compose") {
            Ok(InflectionBody::Compose(self.parse_compose_body()?))
        } else {
            let apply = if self.at_ident("apply") {
                self.advance(); // consume "apply"
                Some(self.parse_apply_expr()?)
            } else {
                None
            };
            let rules = self.parse_rule_list()?;
            Ok(InflectionBody::Rules(RulesBody { apply, rules }))
        }
    }

    /// Parse an apply expression: `harmony(elision(cell))` or `cell`.
    fn parse_apply_expr(&mut self) -> Result<ApplyExpr, Diagnostic> {
        if self.at_ident("cell") {
            self.advance();
            Ok(ApplyExpr::Cell)
        } else if let TokenKind::Ident(_) = self.peek() {
            let rule = self.expect_ident()?;
            self.expect(&TokenKind::LParen)?;
            let inner = self.parse_apply_expr()?;
            self.expect(&TokenKind::RParen)?;
            Ok(ApplyExpr::PhonApply { rule, inner: Box::new(inner) })
        } else {
            Err(Diagnostic::error("expected phonrule name or 'cell' in apply expression")
                .with_label(self.current_span(), "expected phonrule name or 'cell'"))
        }
    }

    fn parse_rule_list(&mut self) -> Result<Vec<InflectionRule>, Diagnostic> {
        let mut rules = Vec::new();
        while matches!(self.peek(), TokenKind::LBracket) {
            rules.push(self.parse_inflection_rule()?);
        }
        Ok(rules)
    }

    fn parse_inflection_rule(&mut self) -> Result<InflectionRule, Diagnostic> {
        let condition = self.parse_tag_condition_list()?;
        self.expect(&TokenKind::Arrow)?;
        let rhs_start = self.current_span().start;
        let rhs = self.parse_rule_rhs()?;
        Ok(InflectionRule {
            condition,
            rhs: Spanned::new(rhs, self.span_from(rhs_start)),
        })
    }

    fn parse_rule_rhs(&mut self) -> Result<RuleRhs, Diagnostic> {
        match self.peek() {
            TokenKind::TemplateLit(_) => {
                let tok = self.advance();
                if let TokenKind::TemplateLit(segs) = tok.node {
                    let template = self.segs_to_template(segs, tok.span);
                    Ok(RuleRhs::Template(template))
                } else { unreachable!() }
            }
            TokenKind::Ident(s) if s == "null" => {
                self.advance();
                Ok(RuleRhs::Null)
            }
            TokenKind::Ident(_) => {
                // Look ahead: Ident + '(' = PhonApply, Ident + '[' = Delegate
                if self.is_next_lparen() {
                    let rule = self.expect_ident()?;
                    self.expect(&TokenKind::LParen)?;
                    let inner_start = self.current_span().start;
                    let inner = self.parse_rule_rhs()?;
                    self.expect(&TokenKind::RParen)?;
                    Ok(RuleRhs::PhonApply {
                        rule,
                        inner: Box::new(Spanned::new(inner, self.span_from(inner_start))),
                    })
                } else {
                    // Delegation
                    Ok(RuleRhs::Delegate(self.parse_delegate()?))
                }
            }
            _ => Err(self.error(format!(
                "expected template, 'null', or delegation, found {:?}",
                self.peek()
            ))),
        }
    }

    /// Check if the token after the current one is LParen.
    fn is_next_lparen(&self) -> bool {
        self.tokens.get(self.pos + 1)
            .map(|t| matches!(t.node, TokenKind::LParen))
            .unwrap_or(false)
    }

    fn segs_to_template(&self, segs: Vec<TemplateSeg>, span: Span) -> Template {
        let segments = segs
            .into_iter()
            .map(|seg| match seg {
                TemplateSeg::Lit(s) => TemplateSegment::Lit(s),
                TemplateSeg::Interp(name) => {
                    let ident = Spanned::new(name, span);
                    TemplateSegment::Stem(ident)
                }
                TemplateSeg::SlotInterp { stem, slot } => TemplateSegment::Slot {
                    stem: Spanned::new(stem, span),
                    slot: Spanned::new(slot, span),
                },
            })
            .collect();
        Template { segments, span }
    }

    fn parse_delegate(&mut self) -> Result<Delegate, Diagnostic> {
        let target = self.expect_ident()?;
        self.expect(&TokenKind::LBracket)?;
        let mut tags = Vec::new();
        while !matches!(self.peek(), TokenKind::RBracket | TokenKind::Eof) {
            let ident = self.expect_ident()?;
            if matches!(self.peek(), TokenKind::Eq) {
                self.advance();
                let value = self.expect_ident()?;
                tags.push(DelegateTag::Fixed(TagCondition {
                    axis: ident,
                    value,
                }));
            } else {
                tags.push(DelegateTag::PassThrough(ident));
            }
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
        self.expect(&TokenKind::RBracket)?;

        let stem_mapping = if self.at_ident("with") {
            self.advance();
            self.expect_keyword("stems")?;
            self.expect(&TokenKind::LBrace)?;
            let mut mappings = Vec::new();
            while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
                let target_stem = self.expect_ident()?;
                self.expect(&TokenKind::Colon)?;
                let source = if matches!(self.peek(), TokenKind::StringLit(_)) {
                    StemSource::Literal(self.expect_string()?)
                } else {
                    StemSource::Stem(self.expect_ident()?)
                };
                mappings.push(StemMapping {
                    target_stem,
                    source,
                });
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
            }
            self.expect(&TokenKind::RBrace)?;
            mappings
        } else {
            Vec::new()
        };

        Ok(Delegate {
            target,
            tags,
            stem_mapping,
        })
    }

    fn parse_compose_body(&mut self) -> Result<ComposeBody, Diagnostic> {
        self.expect_keyword("compose")?;

        // Parse compose expression: harmony(root + sfx1 + sfx2) or root + sfx1 + sfx2
        let chain = self.parse_compose_expr()?;

        let mut slots = Vec::new();
        let mut overrides = Vec::new();

        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            if self.at_ident("slot") {
                self.advance();
                let name = self.expect_ident()?;
                self.expect(&TokenKind::LBrace)?;
                let rules = self.parse_rule_list()?;
                self.expect(&TokenKind::RBrace)?;
                slots.push(SlotDef { name, rules });
            } else if self.at_ident("override") {
                self.advance();
                overrides.push(self.parse_inflection_rule()?);
            } else {
                return Err(self.error(format!(
                    "expected 'slot' or 'override', found {:?}",
                    self.peek()
                )));
            }
        }

        Ok(ComposeBody {
            chain,
            slots,
            overrides,
        })
    }

    /// Parse a compose expression.
    /// ```text
    /// compose_expr = compose_term ('+' compose_term)*
    /// compose_term = IDENT '(' compose_expr ')'   // PhonApply
    ///              | IDENT                          // Slot
    /// ```
    fn parse_compose_expr(&mut self) -> Result<ComposeExpr, Diagnostic> {
        let mut terms = Vec::new();
        terms.push(self.parse_compose_term()?);
        while matches!(self.peek(), TokenKind::Plus) {
            self.advance();
            terms.push(self.parse_compose_term()?);
        }
        if terms.len() == 1 {
            Ok(terms.pop().unwrap())
        } else {
            Ok(ComposeExpr::Concat(terms))
        }
    }

    fn parse_compose_term(&mut self) -> Result<ComposeExpr, Diagnostic> {
        let ident = self.expect_ident()?;
        if matches!(self.peek(), TokenKind::LParen) {
            // PhonApply: name(inner_expr)
            self.advance();
            let inner = self.parse_compose_expr()?;
            self.expect(&TokenKind::RParen)?;
            Ok(ComposeExpr::PhonApply {
                rule: ident,
                inner: Box::new(inner),
            })
        } else {
            Ok(ComposeExpr::Slot(ident))
        }
    }

    // -----------------------------------------------------------------------
    // phonrule
    // -----------------------------------------------------------------------

    fn parse_phonrule(&mut self) -> Result<PhonRule, Diagnostic> {
        let start = self.current_span().start;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut classes = Vec::new();
        let mut maps = Vec::new();
        let mut rules = Vec::new();

        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            if self.at_ident("class") {
                classes.push(self.parse_char_class()?);
            } else if self.at_ident("map") {
                maps.push(self.parse_phon_map()?);
            } else {
                // Must be a rewrite rule
                rules.push(self.parse_phon_rewrite_rule()?);
            }
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(PhonRule {
            name,
            classes,
            maps,
            rules,
            span: self.span_from(start),
        })
    }

    /// Parse `class NAME = ["a", "b"] | class NAME = A | B`
    fn parse_char_class(&mut self) -> Result<CharClassDef, Diagnostic> {
        self.expect_keyword("class")?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Eq)?;

        if matches!(self.peek(), TokenKind::LBracket) {
            // List form: ["a", "b", "c"]
            self.advance();
            let mut list = Vec::new();
            while !matches!(self.peek(), TokenKind::RBracket | TokenKind::Eof) {
                list.push(self.expect_string()?);
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
            }
            self.expect(&TokenKind::RBracket)?;
            Ok(CharClassDef {
                name,
                body: CharClassBody::List(list),
            })
        } else {
            // Union form: A | B | C
            let mut members = Vec::new();
            members.push(self.expect_ident()?);
            while matches!(self.peek(), TokenKind::Pipe) {
                self.advance();
                members.push(self.expect_ident()?);
            }
            Ok(CharClassDef {
                name,
                body: CharClassBody::Union(members),
            })
        }
    }

    /// Parse `map NAME = param -> match { "a" -> "b", else -> param }`
    fn parse_phon_map(&mut self) -> Result<PhonMapDef, Diagnostic> {
        self.expect_keyword("map")?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Eq)?;
        let param = self.expect_ident()?;
        self.expect(&TokenKind::Arrow)?;
        self.expect_keyword("match")?;
        self.expect(&TokenKind::LBrace)?;

        let mut arms = Vec::new();
        let mut else_arm = None;

        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            if self.at_ident("else") {
                self.advance();
                self.expect(&TokenKind::Arrow)?;
                else_arm = Some(self.parse_phon_map_else()?);
                // Allow trailing comma
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
                break;
            }
            let from = self.expect_string()?;
            self.expect(&TokenKind::Arrow)?;
            let to = self.parse_phon_map_result()?;
            arms.push(PhonMapArm { from, to });
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(PhonMapDef {
            name,
            param,
            body: PhonMapBody::Match { arms, else_arm },
        })
    }

    fn parse_phon_map_result(&mut self) -> Result<PhonMapResult, Diagnostic> {
        match self.peek() {
            TokenKind::StringLit(_) => {
                let s = self.expect_string()?;
                Ok(PhonMapResult::Literal(s))
            }
            TokenKind::Ident(_) => {
                let id = self.expect_ident()?;
                Ok(PhonMapResult::Var(id))
            }
            _ => Err(self.error(format!(
                "expected string literal or identifier in map result, found {:?}",
                self.peek()
            ))),
        }
    }

    fn parse_phon_map_else(&mut self) -> Result<PhonMapElse, Diagnostic> {
        match self.peek() {
            TokenKind::StringLit(_) => {
                let s = self.expect_string()?;
                Ok(PhonMapElse::Literal(s))
            }
            TokenKind::Ident(_) => {
                let id = self.expect_ident()?;
                Ok(PhonMapElse::Var(id))
            }
            _ => Err(self.error(format!(
                "expected string literal or identifier in else arm, found {:?}",
                self.peek()
            ))),
        }
    }

    /// Parse a rewrite rule: `FROM -> TO` or `FROM -> TO / CONTEXT`
    fn parse_phon_rewrite_rule(&mut self) -> Result<PhonRewriteRule, Diagnostic> {
        let start = self.current_span().start;

        // FROM: class name or string literal
        let from = match self.peek() {
            TokenKind::StringLit(_) => {
                let s = self.expect_string()?;
                PhonPattern::Literal(s)
            }
            TokenKind::Ident(_) => {
                let id = self.expect_ident()?;
                PhonPattern::Class(id)
            }
            _ => return Err(self.error(format!(
                "expected class name or string literal in rewrite rule, found {:?}",
                self.peek()
            ))),
        };

        self.expect(&TokenKind::Arrow)?;

        // TO: map name, string literal, or "null"
        let to = match self.peek() {
            TokenKind::StringLit(_) => {
                let s = self.expect_string()?;
                PhonReplacement::Literal(s)
            }
            TokenKind::Ident(s) if s == "null" => {
                self.advance();
                PhonReplacement::Null
            }
            TokenKind::Ident(_) => {
                let id = self.expect_ident()?;
                PhonReplacement::Map(id)
            }
            _ => return Err(self.error(format!(
                "expected map name, string literal, or 'null' in rewrite rule, found {:?}",
                self.peek()
            ))),
        };

        // Optional context: / LEFT _ RIGHT
        let context = if matches!(self.peek(), TokenKind::Slash) {
            self.advance();
            Some(self.parse_phon_context()?)
        } else {
            None
        };

        Ok(PhonRewriteRule {
            from,
            to,
            context,
            span: self.span_from(start),
        })
    }

    /// Parse phonological context: elements before `_` and elements after `_`.
    fn parse_phon_context(&mut self) -> Result<PhonContext, Diagnostic> {
        let mut left = Vec::new();
        let mut right = Vec::new();
        let mut seen_underscore = false;

        loop {
            // Check for end of context (next rewrite rule, class/map keyword, or })
            match self.peek() {
                TokenKind::RBrace | TokenKind::Eof => break,
                // If we see an ident followed by ->, it's the start of a new rewrite rule
                TokenKind::Ident(s) if matches!(s.as_str(), "class" | "map") => break,
                TokenKind::Ident(_) => {
                    // Check if this is the start of a new rewrite rule (ident -> ...)
                    if !seen_underscore && self.is_new_rewrite_rule_ahead() {
                        break;
                    }
                    if seen_underscore && self.is_new_rewrite_rule_ahead() {
                        break;
                    }
                }
                TokenKind::StringLit(_) => {
                    // Check if string -> ... (new rewrite rule)
                    if self.is_next_arrow() {
                        break;
                    }
                }
                _ => {}
            }

            if matches!(self.peek(), TokenKind::Underscore) {
                self.advance();
                seen_underscore = true;
                continue;
            }

            let elem = self.parse_phon_context_elem()?;
            if seen_underscore {
                right.push(elem);
            } else {
                left.push(elem);
            }
        }

        Ok(PhonContext { left, right })
    }

    /// Check if current ident position looks like the start of a new rewrite rule.
    fn is_new_rewrite_rule_ahead(&self) -> bool {
        // IDENT -> ... (new rewrite rule)
        self.tokens.get(self.pos + 1)
            .map(|t| matches!(t.node, TokenKind::Arrow))
            .unwrap_or(false)
    }

    /// Check if the next token is Arrow.
    fn is_next_arrow(&self) -> bool {
        self.tokens.get(self.pos + 1)
            .map(|t| matches!(t.node, TokenKind::Arrow))
            .unwrap_or(false)
    }

    fn parse_phon_context_elem(&mut self) -> Result<PhonContextElem, Diagnostic> {
        let elem = match self.peek() {
            TokenKind::Plus => {
                self.advance();
                return Ok(PhonContextElem::Boundary);
            }
            TokenKind::Caret => {
                self.advance();
                return Ok(PhonContextElem::WordStart);
            }
            TokenKind::Dollar => {
                self.advance();
                return Ok(PhonContextElem::WordEnd);
            }
            TokenKind::LParen => {
                self.advance();
                let mut alts = vec![self.parse_phon_context_elem()?];
                while matches!(self.peek(), TokenKind::Pipe) {
                    self.advance();
                    alts.push(self.parse_phon_context_elem()?);
                }
                self.expect(&TokenKind::RParen)?;
                PhonContextElem::Alt(alts)
            }
            TokenKind::Bang => {
                self.advance();
                let id = self.expect_ident()?;
                PhonContextElem::NegClass(id)
            }
            TokenKind::StringLit(_) => {
                let s = self.expect_string()?;
                PhonContextElem::Literal(s)
            }
            TokenKind::Ident(_) => {
                let id = self.expect_ident()?;
                PhonContextElem::Class(id)
            }
            _ => {
                return Err(self.error(format!(
                    "expected context element, found {:?}",
                    self.peek()
                )));
            }
        };

        // Check for * (repeat)
        if matches!(self.peek(), TokenKind::Star) {
            self.advance();
            Ok(PhonContextElem::Repeat(Box::new(elem)))
        } else {
            Ok(elem)
        }
    }

    // -----------------------------------------------------------------------
    // entry
    // -----------------------------------------------------------------------

    fn parse_entry(&mut self) -> Result<Entry, Diagnostic> {
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut headword = None;
        let mut tags = Vec::new();
        let mut stems = Vec::new();
        let mut inflection = None;
        let mut meaning = None;
        let mut forms_override = Vec::new();
        let mut etymology = None;
        let mut examples = Vec::new();

        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            if self.at_ident("headword") {
                self.advance();
                headword = Some(self.parse_headword()?);
            } else if self.at_ident("tags") {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                tags = self.parse_tag_condition_list_plain()?;
            } else if self.at_ident("stems") {
                self.advance();
                stems = self.parse_stem_defs()?;
            } else if self.at_ident("inflection_class") {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                let class = self.expect_ident()?;
                inflection = Some(EntryInflection::Class(class));
            } else if self.at_ident("inflect") {
                self.advance();
                self.expect_keyword("for")?;
                let axes = self.parse_axis_list()?;
                self.expect(&TokenKind::LBrace)?;
                let body = self.parse_inflection_body()?;
                self.expect(&TokenKind::RBrace)?;
                inflection = Some(EntryInflection::Inline(InlineInflection { axes, body }));
            } else if self.at_ident("meaning") {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                let text = self.expect_string()?;
                meaning = Some(MeaningDef::Single(text));
            } else if self.at_ident("meanings") {
                self.advance();
                meaning = Some(self.parse_meanings()?);
            } else if self.at_ident("forms_override") {
                self.advance();
                self.expect(&TokenKind::LBrace)?;
                forms_override = self.parse_rule_list()?;
                self.expect(&TokenKind::RBrace)?;
            } else if self.at_ident("etymology") {
                self.advance();
                etymology = Some(self.parse_etymology()?);
            } else if self.at_ident("examples") {
                self.advance();
                examples = self.parse_examples()?;
            } else {
                return Err(self.error(format!(
                    "unexpected field in entry: {:?}",
                    self.peek()
                )));
            }
        }

        self.expect(&TokenKind::RBrace)?;

        let headword =
            headword.ok_or_else(|| self.error("entry missing 'headword' field"))?;
        let meaning =
            meaning.ok_or_else(|| self.error("entry missing 'meaning' or 'meanings' field"))?;

        Ok(Entry {
            name,
            headword,
            tags,
            stems,
            inflection,
            meaning,
            forms_override,
            etymology,
            examples,
        })
    }

    fn parse_headword(&mut self) -> Result<Headword, Diagnostic> {
        if matches!(self.peek(), TokenKind::Colon) {
            // Simple headword: `headword: "text"`
            self.advance();
            let s = self.expect_string()?;
            Ok(Headword::Simple(s))
        } else if matches!(self.peek(), TokenKind::LBrace) {
            // Multi-script: `headword { default: "食べる", kana: "たべる" }`
            self.expect(&TokenKind::LBrace)?;
            let mut scripts = Vec::new();
            while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
                let key = self.expect_ident()?;
                self.expect(&TokenKind::Colon)?;
                let value = self.expect_string()?;
                scripts.push((key, value));
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
            }
            self.expect(&TokenKind::RBrace)?;
            Ok(Headword::MultiScript(scripts))
        } else {
            Err(self.error("expected ':' or '{' after 'headword'"))
        }
    }

    fn parse_tag_condition_list_plain(&mut self) -> Result<Vec<TagCondition>, Diagnostic> {
        self.expect(&TokenKind::LBracket)?;
        let mut conditions = Vec::new();
        while !matches!(self.peek(), TokenKind::RBracket | TokenKind::Eof) {
            let axis = self.expect_ident()?;
            self.expect(&TokenKind::Eq)?;
            let value = self.expect_ident()?;
            conditions.push(TagCondition { axis, value });
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
        self.expect(&TokenKind::RBracket)?;
        Ok(conditions)
    }

    fn parse_stem_defs(&mut self) -> Result<Vec<StemDef>, Diagnostic> {
        self.expect(&TokenKind::LBrace)?;
        let mut stems = Vec::new();
        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            let name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let value = self.expect_string()?;
            stems.push(StemDef { name, value });
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(stems)
    }

    fn parse_meanings(&mut self) -> Result<MeaningDef, Diagnostic> {
        self.expect(&TokenKind::LBrace)?;
        let mut entries = Vec::new();
        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            let ident = self.expect_ident()?;
            self.expect(&TokenKind::LBrace)?;
            let text = self.expect_string()?;
            self.expect(&TokenKind::RBrace)?;
            entries.push(MeaningEntry { ident, text });
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(MeaningDef::Multiple(entries))
    }

    // -----------------------------------------------------------------------
    // etymology
    // -----------------------------------------------------------------------

    fn parse_etymology(&mut self) -> Result<Etymology, Diagnostic> {
        self.expect(&TokenKind::LBrace)?;

        let mut proto = None;
        let mut cognates = Vec::new();
        let mut derived_from = None;
        let mut note = None;

        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            if self.at_ident("proto") {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                proto = Some(self.expect_string()?);
            } else if self.at_ident("cognates") {
                self.advance();
                cognates = self.parse_cognates()?;
            } else if self.at_ident("derived_from") {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                derived_from = Some(self.parse_entry_ref()?);
            } else if self.at_ident("note") {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                note = Some(self.expect_string()?);
            } else {
                return Err(self.error(format!(
                    "unexpected field in etymology: {:?}",
                    self.peek()
                )));
            }
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(Etymology {
            proto,
            cognates,
            derived_from,
            note,
        })
    }

    fn parse_cognates(&mut self) -> Result<Vec<Cognate>, Diagnostic> {
        self.expect(&TokenKind::LBrace)?;
        let mut cognates = Vec::new();
        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            let entry = self.parse_entry_ref()?;
            let note = self.expect_string()?;
            cognates.push(Cognate { entry, note });
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(cognates)
    }

    // -----------------------------------------------------------------------
    // examples
    // -----------------------------------------------------------------------

    fn parse_examples(&mut self) -> Result<Vec<Example>, Diagnostic> {
        self.expect(&TokenKind::LBrace)?;
        let mut examples = Vec::new();
        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            self.expect_keyword("example")?;
            examples.push(self.parse_example()?);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(examples)
    }

    fn parse_example(&mut self) -> Result<Example, Diagnostic> {
        self.expect(&TokenKind::LBrace)?;

        let mut tokens = Vec::new();
        let mut translation = None;

        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            if self.at_ident("tokens") {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                tokens = self.parse_token_list()?;
            } else if self.at_ident("translation") {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                translation = Some(self.expect_string()?);
            } else {
                return Err(self.error(format!(
                    "unexpected field in example: {:?}",
                    self.peek()
                )));
            }
        }

        self.expect(&TokenKind::RBrace)?;

        let translation =
            translation.ok_or_else(|| self.error("example missing 'translation'"))?;

        Ok(Example {
            tokens,
            translation,
        })
    }

    fn parse_token_list(&mut self) -> Result<Vec<crate::ast::Token>, Diagnostic> {
        let mut tokens = Vec::new();
        // tokens: entry_ref[form_spec] "literal" entry_ref2[] "."
        // Use ~ between tokens to suppress separator (glue).
        // Ends at the next field keyword (translation) or }
        loop {
            match self.peek() {
                TokenKind::StringLit(_) => {
                    let s = self.expect_string()?;
                    tokens.push(crate::ast::Token::Lit(s));
                }
                TokenKind::Ident(_) => {
                    // Could be an entry ref or a keyword like "translation"
                    if self.at_ident("translation") {
                        break;
                    }
                    let entry_ref = self.parse_entry_ref()?;
                    tokens.push(crate::ast::Token::Ref(entry_ref));
                }
                TokenKind::Tilde => {
                    self.advance();
                    tokens.push(crate::ast::Token::Glue);
                }
                TokenKind::DoubleSlash => {
                    self.advance();
                    tokens.push(crate::ast::Token::Newline);
                }
                _ => break,
            }
        }
        Ok(tokens)
    }

    /// Parse a `.hut` file until EOF: leading `@reference` directives, then a token list.
    pub fn parse_token_list_to_eof(mut self) -> (crate::ast::HutFile, Vec<Diagnostic>) {
        // Parse leading @reference directives
        let mut references = Vec::new();
        while matches!(self.peek(), TokenKind::AtReference) {
            self.advance();
            match self.parse_import() {
                Ok(import) => references.push(import),
                Err(diag) => self.errors.push(diag),
            }
        }

        let tokens = self.parse_hut_tokens(None);
        (crate::ast::HutFile { references, tokens }, self.errors)
    }

    /// Parse hut tokens. If `inside_tag` is Some, stop at the matching `</name>`;
    /// otherwise parse until EOF.
    fn parse_hut_tokens(&mut self, inside_tag: Option<&str>) -> Vec<crate::ast::Token> {
        let mut tokens = Vec::new();
        loop {
            if self.at_eof() {
                if let Some(tag) = inside_tag {
                    self.errors.push(self.error(format!(
                        "unclosed tag <{}>: expected </{}>",
                        tag, tag
                    )));
                }
                break;
            }

            // Check for closing tag </name>
            if matches!(self.peek(), TokenKind::Lt) {
                if self.tokens.get(self.pos + 1)
                    .map(|t| matches!(t.node, TokenKind::Slash))
                    .unwrap_or(false)
                {
                    // Consume < and /
                    self.advance(); // <
                    self.advance(); // /
                    let close_name = if matches!(self.peek(), TokenKind::Ident(_)) {
                        self.parse_hyphenated_name()
                    } else {
                        "?".to_string()
                    };
                    if let Some(tag) = inside_tag {
                        if close_name == tag {
                            if matches!(self.peek(), TokenKind::Gt) {
                                self.advance(); // >
                            } else {
                                self.errors.push(self.error(format!(
                                    "expected '>' after </{}>",
                                    tag
                                )));
                            }
                            break;
                        } else {
                            self.errors.push(self.error(format!(
                                "mismatched closing tag: expected </{}>, found </{}>",
                                tag, close_name
                            )));
                            if matches!(self.peek(), TokenKind::Gt) {
                                self.advance(); // >
                            }
                            continue;
                        }
                    } else {
                        self.errors.push(self.error(format!(
                            "unexpected closing tag </{}> without matching opening tag",
                            close_name
                        )));
                        if matches!(self.peek(), TokenKind::Gt) { self.advance(); }
                        continue;
                    }
                }
            }

            match self.peek() {
                TokenKind::StringLit(_) => {
                    let s = self.expect_string().unwrap();
                    tokens.push(crate::ast::Token::Lit(s));
                }
                TokenKind::Ident(_) => {
                    match self.parse_entry_ref() {
                        Ok(entry_ref) => tokens.push(crate::ast::Token::Ref(entry_ref)),
                        Err(diag) => {
                            self.errors.push(diag);
                            self.advance();
                        }
                    }
                }
                TokenKind::Tilde => {
                    self.advance();
                    tokens.push(crate::ast::Token::Glue);
                }
                TokenKind::DoubleSlash => {
                    self.advance();
                    tokens.push(crate::ast::Token::Newline);
                }
                TokenKind::Lt => {
                    let start = self.current_span().start;
                    self.advance(); // <
                    // Expect tag name (Ident(-Ident)* for custom elements)
                    match self.peek() {
                        TokenKind::Ident(_) => {
                            let tag_name = self.parse_hyphenated_name();
                            let attrs = self.parse_tag_attrs();
                            match self.peek() {
                                TokenKind::Slash => {
                                    // Self-closing: <name attrs/>
                                    self.advance(); // /
                                    if matches!(self.peek(), TokenKind::Gt) {
                                        self.advance(); // >
                                    } else {
                                        self.errors.push(self.error(format!(
                                            "expected '>' after <{}/>",
                                            tag_name
                                        )));
                                    }
                                    tokens.push(crate::ast::Token::SelfClosingTag {
                                        name: tag_name,
                                        attrs,
                                        span: self.span_from(start),
                                    });
                                }
                                TokenKind::Gt => {
                                    // Open tag: <name attrs> ... </name>
                                    self.advance(); // >
                                    let children = self.parse_hut_tokens(Some(&tag_name));
                                    tokens.push(crate::ast::Token::Tag {
                                        name: tag_name,
                                        attrs,
                                        children,
                                        span: self.span_from(start),
                                    });
                                }
                                _ => {
                                    self.errors.push(self.error(format!(
                                        "expected '>' or '/>' after <{}",
                                        tag_name
                                    )));
                                }
                            }
                        }
                        _ => {
                            self.errors.push(self.error(
                                "expected tag name after '<'".to_string()
                            ));
                        }
                    }
                }
                _ => {
                    self.errors.push(self.error(format!(
                        "unexpected token in .hut file: {:?}",
                        self.peek()
                    )));
                    self.advance();
                }
            }
        }
        tokens
    }

    fn parse_render_config(&mut self) -> Result<RenderConfig, Diagnostic> {
        self.expect(&TokenKind::LBrace)?;
        let mut separator = None;
        let mut no_separator_before = None;
        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            if self.at_ident("separator") {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                separator = Some(self.expect_string()?);
            } else if self.at_ident("no_separator_before") {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                no_separator_before = Some(self.expect_string()?);
            } else {
                return Err(self.error(format!(
                    "unexpected field in @render: {:?}",
                    self.peek()
                )));
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(RenderConfig {
            separator,
            no_separator_before,
        })
    }

    // -----------------------------------------------------------------------
    // Tag helpers
    // -----------------------------------------------------------------------

    /// Parse a hyphenated tag name: `Ident ( - Ident )*` → `"my-custom-elem"`.
    fn parse_hyphenated_name(&mut self) -> String {
        let mut name = self.expect_ident().unwrap().node;
        while matches!(self.peek(), TokenKind::Minus) {
            self.advance(); // -
            if matches!(self.peek(), TokenKind::Ident(_)) {
                name.push('-');
                name.push_str(&self.expect_ident().unwrap().node);
            } else {
                // Trailing `-` without ident — include it and stop
                name.push('-');
                break;
            }
        }
        name
    }

    /// Parse tag attributes: `( Ident = StringLit )*` until `>` or `/>`.
    fn parse_tag_attrs(&mut self) -> Vec<(String, String)> {
        let mut attrs = Vec::new();
        while matches!(self.peek(), TokenKind::Ident(_)) {
            let attr_name = self.parse_hyphenated_name();
            if matches!(self.peek(), TokenKind::Eq) {
                self.advance(); // =
                match self.peek() {
                    TokenKind::StringLit(_) => {
                        let value = self.expect_string().unwrap().node;
                        attrs.push((attr_name, value));
                    }
                    _ => {
                        self.errors.push(self.error(format!(
                            "expected string value for attribute '{}'",
                            attr_name
                        )));
                        break;
                    }
                }
            } else {
                // Boolean attribute (no value), e.g. <input disabled>
                attrs.push((attr_name, String::new()));
            }
        }
        attrs
    }

    // -----------------------------------------------------------------------
    // EntryRef
    // -----------------------------------------------------------------------

    fn parse_entry_ref(&mut self) -> Result<EntryRef, Diagnostic> {
        let start = self.current_span().start;

        // Collect dot-separated identifiers
        let mut parts = Vec::new();
        parts.push(self.expect_ident()?);
        while matches!(self.peek(), TokenKind::Dot) {
            self.advance();
            parts.push(self.expect_ident()?);
        }

        // Last part is entry_id, rest are namespace
        let entry_id = parts.pop().unwrap();
        let namespace = parts;

        // Optional #meaning
        let meaning = if matches!(self.peek(), TokenKind::Hash) {
            self.advance();
            Some(self.expect_ident()?)
        } else {
            None
        };

        // Optional [form_spec] or [$=stem_name]
        let mut form_spec = None;
        let mut stem_spec = None;
        if matches!(self.peek(), TokenKind::LBracket) {
            // Peek ahead: if next token after `[` is `$`, it's a stem spec
            let is_stem_spec = self.tokens.get(self.pos + 1)
                .map(|t| matches!(t.node, TokenKind::Dollar))
                .unwrap_or(false);
            if is_stem_spec {
                self.advance(); // [
                self.advance(); // $
                self.expect(&TokenKind::Eq)?;
                stem_spec = Some(self.expect_ident()?);
                self.expect(&TokenKind::RBracket)?;
            } else {
                form_spec = Some(self.parse_tag_condition_list()?);
            }
        }

        Ok(EntryRef {
            namespace,
            entry_id,
            meaning,
            form_spec,
            stem_spec,
            span: self.span_from(start),
        })
    }
}

// ---------------------------------------------------------------------------
// Handle `*` for import target
// ---------------------------------------------------------------------------

// We need to handle `*` in the lexer. Let me add it.
// Actually, looking at the spec, `*` in import is like:
//   @use * from "file.hu"
//   @use * as ns from "file.hu"
// The `*` is not a general operator — it only appears in import targets.
// We should handle it at the parser level by checking if ident == "*".
// But `*` won't be lexed as an Ident — it's not XID_Start.
// We need to either add a Star token to the lexer or handle it specially.
// Let me add a Star token.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::span::FileId;

    fn parse_str(input: &str) -> (File, Vec<Diagnostic>) {
        let lexer = Lexer::new(input, FileId(0));
        let (tokens, lex_errors) = lexer.tokenize();
        assert!(lex_errors.is_empty(), "lex errors: {:?}", lex_errors);
        let parser = Parser::new(tokens, FileId(0));
        parser.parse()
    }

    #[test]
    fn test_tagaxis() {
        let (file, errors) = parse_str(
            r#"
            tagaxis tense {
                role: inflectional
                display: { ja: "時制", en: "Tense" }
            }
            "#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(file.items.len(), 1);
        match &file.items[0].node {
            Item::TagAxis(ta) => {
                assert_eq!(ta.name.node, "tense");
                assert_eq!(ta.role.node, Role::Inflectional);
                assert_eq!(ta.display.len(), 2);
            }
            other => panic!("expected TagAxis, got {:?}", other),
        }
    }

    #[test]
    fn test_extend() {
        let (file, errors) = parse_str(
            r#"
            @extend verb_noun for tagaxis parts_of_speech {
                verb { display: { ja: "動詞" } }
                noun { display: { ja: "名詞" } }
            }
            "#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(file.items.len(), 1);
        match &file.items[0].node {
            Item::Extend(ext) => {
                assert_eq!(ext.name.node, "verb_noun");
                assert_eq!(ext.target_axis.node, "parts_of_speech");
                assert_eq!(ext.values.len(), 2);
                assert_eq!(ext.values[0].name.node, "verb");
            }
            other => panic!("expected Extend, got {:?}", other),
        }
    }

    #[test]
    fn test_inflection_simple() {
        let (file, errors) = parse_str(
            r#"
            inflection strong_I for {tense, person, number} {
                requires stems: pres, past

                [tense=present, person=1, number=sg] -> `{pres}e`
                [tense=future, _] -> null
            }
            "#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        match &file.items[0].node {
            Item::Inflection(infl) => {
                assert_eq!(infl.name.node, "strong_I");
                assert_eq!(infl.axes.len(), 3);
                assert_eq!(infl.required_stems.len(), 2);
                match &infl.body {
                    InflectionBody::Rules(body) => {
                        assert!(body.apply.is_none());
                        assert_eq!(body.rules.len(), 2);
                        assert!(!body.rules[0].condition.wildcard);
                        assert!(body.rules[1].condition.wildcard);
                    }
                    _ => panic!("expected Rules body"),
                }
            }
            other => panic!("expected Inflection, got {:?}", other),
        }
    }

    #[test]
    fn test_inflection_compose() {
        let (file, errors) = parse_str(
            r#"
            inflection regular for {tense, person} {
                requires stems: root

                compose root + tense_sfx + pn_sfx

                slot tense_sfx {
                    [tense=present] -> ``
                    [tense=past] -> `ta`
                }

                slot pn_sfx {
                    [person=1] -> `m`
                    [person=2] -> `n`
                }

                override [tense=past, person=1] -> `{root}tta`
            }
            "#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        match &file.items[0].node {
            Item::Inflection(infl) => match &infl.body {
                InflectionBody::Compose(comp) => {
                    // chain should be Concat of 3 Slots
                    match &comp.chain {
                        ComposeExpr::Concat(terms) => assert_eq!(terms.len(), 3),
                        _ => panic!("expected Concat compose expr"),
                    }
                    assert_eq!(comp.slots.len(), 2);
                    assert_eq!(comp.overrides.len(), 1);
                }
                _ => panic!("expected Compose body"),
            },
            other => panic!("expected Inflection, got {:?}", other),
        }
    }

    #[test]
    fn test_inflection_delegate() {
        let (file, errors) = parse_str(
            r#"
            inflection adj for {case, number, gender} {
                requires stems: nom_m, nom_f

                [gender=fem, _] -> first_decl[case, number] with stems { nom: nom_f }
                [gender=masc, _] -> second_decl[case, number] with stems { nom: nom_m }
            }
            "#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        match &file.items[0].node {
            Item::Inflection(infl) => match &infl.body {
                InflectionBody::Rules(body) => {
                    assert_eq!(body.rules.len(), 2);
                    match &body.rules[0].rhs.node {
                        RuleRhs::Delegate(d) => {
                            assert_eq!(d.target.node, "first_decl");
                            assert_eq!(d.tags.len(), 2);
                            assert_eq!(d.stem_mapping.len(), 1);
                        }
                        other => panic!("expected Delegate, got {:?}", other),
                    }
                }
                _ => panic!("expected Rules body"),
            },
            other => panic!("expected Inflection, got {:?}", other),
        }
    }

    #[test]
    fn test_entry_simple() {
        let (file, errors) = parse_str(
            r#"
            entry faren {
                headword: "faren"
                tags: [parts_of_speech=verb]
                stems { pres: "far", past: "for" }
                inflection_class: strong_I
                meaning: "to go"
            }
            "#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        match &file.items[0].node {
            Item::Entry(e) => {
                assert_eq!(e.name.node, "faren");
                assert!(matches!(&e.headword, Headword::Simple(s) if s.node == "faren"));
                assert_eq!(e.tags.len(), 1);
                assert_eq!(e.stems.len(), 2);
                assert!(matches!(&e.inflection, Some(EntryInflection::Class(c)) if c.node == "strong_I"));
                assert!(matches!(&e.meaning, MeaningDef::Single(s) if s.node == "to go"));
            }
            other => panic!("expected Entry, got {:?}", other),
        }
    }

    #[test]
    fn test_entry_with_etymology() {
        let (file, errors) = parse_str(
            r#"
            entry faren {
                headword: "faren"
                tags: [parts_of_speech=verb]
                stems {}
                meaning: "to go"
                etymology {
                    proto: "*far-"
                    cognates {
                        afaran "prefixed derivative"
                    }
                    derived_from: proto_far
                    note: "Underwent i-umlaut."
                }
            }
            "#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        match &file.items[0].node {
            Item::Entry(e) => {
                let ety = e.etymology.as_ref().unwrap();
                assert_eq!(ety.proto.as_ref().unwrap().node, "*far-");
                assert_eq!(ety.cognates.len(), 1);
                assert_eq!(ety.derived_from.as_ref().unwrap().entry_id.node, "proto_far");
                assert_eq!(ety.note.as_ref().unwrap().node, "Underwent i-umlaut.");
            }
            other => panic!("expected Entry, got {:?}", other),
        }
    }

    #[test]
    fn test_entry_with_examples() {
        let (file, errors) = parse_str(
            r#"
            entry faren {
                headword: "faren"
                tags: [parts_of_speech=verb]
                stems {}
                meaning: "to go"
                examples {
                    example {
                        tokens: faren[tense=present, person=1] "."
                        translation: "I go."
                    }
                }
            }
            "#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        match &file.items[0].node {
            Item::Entry(e) => {
                assert_eq!(e.examples.len(), 1);
                assert_eq!(e.examples[0].tokens.len(), 2);
                assert_eq!(e.examples[0].translation.node, "I go.");
            }
            other => panic!("expected Entry, got {:?}", other),
        }
    }

    #[test]
    fn test_token_list_with_glue() {
        let (file, errors) = parse_str(
            r#"
            entry malbona {
                headword: "malbona"
                tags: [pos=adj]
                stems {}
                meaning: "bad"
                examples {
                    example {
                        tokens: "mal"~"bona" "hundo"
                        translation: "bad dog"
                    }
                }
            }
            "#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        match &file.items[0].node {
            Item::Entry(e) => {
                assert_eq!(e.examples.len(), 1);
                // "mal", Glue, "bona", "hundo" = 4 tokens
                assert_eq!(e.examples[0].tokens.len(), 4);
                assert!(matches!(e.examples[0].tokens[1], crate::ast::Token::Glue));
            }
            other => panic!("expected Entry, got {:?}", other),
        }
    }

    #[test]
    fn test_entry_ref_stem_spec() {
        let (file, errors) = parse_str(
            r#"
            entry gelmek {
                headword: "gelmek"
                tags: [pos=verb]
                stems { root: "gel" }
                meaning: "to come"
                examples {
                    example {
                        tokens: gelmek[$=root]~"iyor"
                        translation: "is coming"
                    }
                }
            }
            "#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        match &file.items[0].node {
            Item::Entry(e) => {
                assert_eq!(e.examples.len(), 1);
                // gelmek[$=root], Glue, "iyor" = 3 tokens
                assert_eq!(e.examples[0].tokens.len(), 3);
                if let crate::ast::Token::Ref(r) = &e.examples[0].tokens[0] {
                    assert!(r.form_spec.is_none());
                    assert_eq!(r.stem_spec.as_ref().unwrap().node, "root");
                } else {
                    panic!("expected Ref token");
                }
                assert!(matches!(e.examples[0].tokens[1], crate::ast::Token::Glue));
            }
            other => panic!("expected Entry, got {:?}", other),
        }
    }

    #[test]
    fn test_entry_inline_inflect() {
        let (file, errors) = parse_str(
            r#"
            entry sein {
                headword: "sein"
                tags: [parts_of_speech=verb]
                stems {}
                meaning: "to be"
                inflect for {tense, person} {
                    [tense=present, person=1] -> `bin`
                    [tense=present, person=2] -> `bist`
                    [_] -> null
                }
            }
            "#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        match &file.items[0].node {
            Item::Entry(e) => {
                assert!(matches!(&e.inflection, Some(EntryInflection::Inline(_))));
            }
            other => panic!("expected Entry, got {:?}", other),
        }
    }

    #[test]
    fn test_use_glob() {
        let (file, errors) = parse_str(
            r#"@use * from "core/tags.hu""#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        match &file.items[0].node {
            Item::Use(imp) => {
                assert!(matches!(&imp.target, ImportTarget::Glob { alias: None }));
                assert_eq!(imp.path.node, "core/tags.hu");
            }
            other => panic!("expected Use, got {:?}", other),
        }
    }

    #[test]
    fn test_use_glob_as() {
        let (file, errors) = parse_str(
            r#"@use * as std from "stdlib/universal.hu""#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        match &file.items[0].node {
            Item::Use(imp) => {
                match &imp.target {
                    ImportTarget::Glob { alias: Some(a) } => assert_eq!(a.node, "std"),
                    other => panic!("expected Glob with alias, got {:?}", other),
                }
            }
            other => panic!("expected Use, got {:?}", other),
        }
    }

    #[test]
    fn test_use_named() {
        let (file, errors) = parse_str(
            r#"@use tense, aspect as a from "core/verbal.hu""#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        match &file.items[0].node {
            Item::Use(imp) => {
                match &imp.target {
                    ImportTarget::Named(entries) => {
                        assert_eq!(entries.len(), 2);
                        assert_eq!(entries[0].name.node, "tense");
                        assert!(entries[0].alias.is_none());
                        assert_eq!(entries[1].name.node, "aspect");
                        assert_eq!(entries[1].alias.as_ref().unwrap().node, "a");
                    }
                    other => panic!("expected Named, got {:?}", other),
                }
            }
            other => panic!("expected Use, got {:?}", other),
        }
    }

    #[test]
    fn test_reference() {
        let (file, errors) = parse_str(
            r#"@reference * as verbs from "entries/verbs.hu""#,
        );
        assert!(errors.is_empty(), "errors: {:?}", errors);
        match &file.items[0].node {
            Item::Reference(imp) => {
                match &imp.target {
                    ImportTarget::Glob { alias: Some(a) } => assert_eq!(a.node, "verbs"),
                    other => panic!("expected Glob with alias, got {:?}", other),
                }
            }
            other => panic!("expected Reference, got {:?}", other),
        }
    }
}
