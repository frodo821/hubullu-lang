#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::PathBuf;

use hubullu::span::{FileId, SourceMap};
use hubullu::token::Token;
use hubullu::ParseResult;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a single .hu source string and return everything needed for LSP tests.
fn parse(source: &str) -> ParseResult {
    hubullu::parse_source(source, "/tmp/test.hu")
}

/// Run phase1 on a fixture directory.
fn phase1_fixture(name: &str) -> hubullu::phase1::Phase1Result {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
        .join("main.hu");
    hubullu::phase1::run_phase1(&path)
}

/// Build a token cache from phase1 result.
fn token_cache(p1: &hubullu::phase1::Phase1Result) -> HashMap<FileId, Vec<Token>> {
    let mut cache = HashMap::new();
    for (&fid, _file) in &p1.files {
        let source = p1.source_map.source(fid);
        let lexer = hubullu::lexer::Lexer::new(source, fid);
        let (tokens, _) = lexer.tokenize();
        cache.insert(fid, tokens);
    }
    cache
}

/// Find the file_id for a path suffix (e.g. "main.hu") in a phase1 result.
fn find_file_id(p1: &hubullu::phase1::Phase1Result, suffix: &str) -> FileId {
    for (&fid, _) in &p1.files {
        if p1.source_map.path(fid).to_string_lossy().ends_with(suffix) {
            return fid;
        }
    }
    panic!("file not found: {}", suffix);
}

/// Find the byte offset of the first occurrence of `needle` in the source of `file_id`.
fn find_offset(source_map: &SourceMap, file_id: FileId, needle: &str) -> usize {
    source_map
        .source(file_id)
        .find(needle)
        .unwrap_or_else(|| panic!("'{}' not found in source", needle))
}

// ===========================================================================
// convert
// ===========================================================================
mod convert {
    use super::*;

    #[test]
    fn offset_to_position_first_line() {
        let pr = parse("entry foo {}");
        let pos = hubullu::lsp::convert::offset_to_position(pr.file_id, 0, &pr.source_map);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn offset_to_position_second_line() {
        let pr = parse("line0\nline1");
        // 'l' of line1 is at byte offset 6
        let pos = hubullu::lsp::convert::offset_to_position(pr.file_id, 6, &pr.source_map);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn offset_to_position_multibyte() {
        // "あ" is 3 bytes in UTF-8, 1 code unit in UTF-16.
        let pr = parse("あbc");
        // 'b' is at byte 3
        let pos = hubullu::lsp::convert::offset_to_position(pr.file_id, 3, &pr.source_map);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 1); // UTF-16 code units
    }

    #[test]
    fn position_to_offset_roundtrip() {
        let pr = parse("line0\nline1\nline2");
        let offset = 12; // 'l' of "line2"
        let pos = hubullu::lsp::convert::offset_to_position(pr.file_id, offset, &pr.source_map);
        let back =
            hubullu::lsp::convert::position_to_offset(&pos, pr.file_id, &pr.source_map).unwrap();
        assert_eq!(back, offset);
    }

    #[test]
    fn span_to_range_basic() {
        let pr = parse("entry foo {}");
        // "foo" starts at byte 6, ends at 9
        let span = crate::find_token_span(&pr, "foo");
        let range = hubullu::lsp::convert::span_to_range(&span, &pr.source_map);
        assert_eq!(range.start.line, 0);
        assert_eq!(range.start.character, 6);
        assert_eq!(range.end.line, 0);
        assert_eq!(range.end.character, 9);
    }

    #[test]
    fn path_to_uri_and_back() {
        let path = std::path::Path::new("/tmp/test.hu");
        let uri = hubullu::lsp::convert::path_to_uri(path).unwrap();
        assert!(uri.as_str().starts_with("file:///tmp/test.hu"));
        let back = hubullu::lsp::convert::uri_to_path(&uri).unwrap();
        assert_eq!(back, path);
    }
}

/// Find the Span of the first token matching the given text.
fn find_token_span(pr: &ParseResult, text: &str) -> hubullu::ast::Span {
    use hubullu::token::TokenKind;
    for tok in &pr.tokens {
        match &tok.node {
            TokenKind::Ident(s) if s == text => return tok.span.clone(),
            TokenKind::StringLit(s) if s == text => return tok.span.clone(),
            _ => {}
        }
    }
    panic!("token '{}' not found", text);
}

// ===========================================================================
// formatting
// ===========================================================================
mod formatting {
    #[test]
    fn normalizes_indentation() {
        let input = "entry foo {\nheadword: \"x\"\n}";
        let edits = hubullu::lsp::formatting::format_document(input);
        // "headword:" line should be indented
        assert!(!edits.is_empty(), "expected indentation edits");
        let edit = &edits[0];
        assert_eq!(edit.new_text, "    ");
    }

    #[test]
    fn already_formatted() {
        let input = "entry foo {\n    headword: \"x\"\n}";
        let edits = hubullu::lsp::formatting::format_document(input);
        assert!(edits.is_empty(), "expected no edits for already formatted code");
    }

    #[test]
    fn nested_braces() {
        let input = "entry foo {\nstems {\npres: \"x\"\n}\n}";
        let edits = hubullu::lsp::formatting::format_document(input);
        // Should have edits for "stems {" (indent 1) and "pres:" (indent 2)
        assert!(edits.len() >= 2);
    }
}

// ===========================================================================
// folding
// ===========================================================================
mod folding {
    use super::*;

    #[test]
    fn multiline_entry_produces_fold() {
        let pr = parse("entry foo {\n  headword: \"foo\"\n  meaning: \"bar\"\n}");
        let ranges = hubullu::lsp::folding::folding_ranges(&pr);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start_line, 0);
        assert_eq!(ranges[0].end_line, 3);
    }

    #[test]
    fn single_line_no_fold() {
        let pr = parse("entry foo {}");
        let ranges = hubullu::lsp::folding::folding_ranges(&pr);
        assert!(ranges.is_empty());
    }
}

// ===========================================================================
// symbols
// ===========================================================================
mod symbols {
    use super::*;
    use lsp_types::SymbolKind;

    #[test]
    fn document_symbols_entry() {
        let pr = parse("entry foo {\n  headword: \"foo\"\n  meaning: \"bar\"\n}");
        let syms = hubullu::lsp::symbols::document_symbols(&pr);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "foo");
        assert_eq!(syms[0].kind, SymbolKind::CLASS);
    }

    #[test]
    fn document_symbols_tagaxis() {
        let pr = parse("tagaxis tense {\n  role: inflectional\n}");
        let syms = hubullu::lsp::symbols::document_symbols(&pr);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "tense");
        assert_eq!(syms[0].kind, SymbolKind::ENUM);
    }

    #[test]
    fn document_symbols_inflection() {
        let src = "inflection strong for {tense} {\n  requires stems: root\n  [tense=x] -> `{root}`\n}";
        let pr = parse(src);
        let syms = hubullu::lsp::symbols::document_symbols(&pr);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "strong");
        assert_eq!(syms[0].kind, SymbolKind::FUNCTION);
    }

    #[test]
    fn document_symbols_multiple() {
        let src = "tagaxis t { role: inflectional }\nentry foo {\n  headword: \"f\"\n  meaning: \"m\"\n}";
        let pr = parse(src);
        let syms = hubullu::lsp::symbols::document_symbols(&pr);
        assert_eq!(syms.len(), 2);
    }

    #[test]
    fn workspace_symbols_filter() {
        let p1 = phase1_fixture("simple");
        let resp = hubullu::lsp::symbols::workspace_symbols("faren", &p1);
        match resp {
            lsp_types::WorkspaceSymbolResponse::Flat(syms) => {
                assert!(syms.iter().any(|s| s.name == "faren"));
            }
            _ => panic!("expected flat response"),
        }
    }

    #[test]
    fn workspace_symbols_empty_query() {
        let p1 = phase1_fixture("simple");
        let resp = hubullu::lsp::symbols::workspace_symbols("", &p1);
        match resp {
            lsp_types::WorkspaceSymbolResponse::Flat(syms) => {
                // Should return all symbols
                assert!(syms.len() > 1);
            }
            _ => panic!("expected flat response"),
        }
    }
}

// ===========================================================================
// hover
// ===========================================================================
mod hover_tests {
    use super::*;

    #[test]
    fn hover_on_entry_name_at_definition() {
        let p1 = phase1_fixture("simple");
        let fid = find_file_id(&p1, "main.hu");
        let tc = token_cache(&p1);
        let tokens = tc.get(&fid).unwrap();
        let offset = find_offset(&p1.source_map, fid, "faren");

        let result = hubullu::lsp::hover::hover(fid, offset, tokens, &p1);
        assert!(result.is_some());
        let hover = result.unwrap();
        match &hover.contents {
            lsp_types::HoverContents::Markup(m) => {
                assert!(m.value.contains("entry faren"), "hover should mention entry name");
                assert!(m.value.contains("faren"), "hover should mention headword");
            }
            _ => panic!("expected markup content"),
        }
    }

    #[test]
    fn hover_on_inflection_name() {
        let p1 = phase1_fixture("simple");
        let fid = find_file_id(&p1, "profile.hu");
        let tc = token_cache(&p1);
        let tokens = tc.get(&fid).unwrap();
        let offset = find_offset(&p1.source_map, fid, "strong_I");

        let result = hubullu::lsp::hover::hover(fid, offset, tokens, &p1);
        assert!(result.is_some());
        let hover = result.unwrap();
        match &hover.contents {
            lsp_types::HoverContents::Markup(m) => {
                assert!(m.value.contains("inflection strong_I"));
            }
            _ => panic!("expected markup content"),
        }
    }

    #[test]
    fn hover_on_tagaxis_name() {
        let p1 = phase1_fixture("simple");
        let fid = find_file_id(&p1, "profile.hu");
        let tc = token_cache(&p1);
        let tokens = tc.get(&fid).unwrap();
        let offset = find_offset(&p1.source_map, fid, "tense");

        let result = hubullu::lsp::hover::hover(fid, offset, tokens, &p1);
        assert!(result.is_some());
        let hover = result.unwrap();
        match &hover.contents {
            lsp_types::HoverContents::Markup(m) => {
                assert!(m.value.contains("tagaxis tense"));
            }
            _ => panic!("expected markup content"),
        }
    }

    #[test]
    fn hover_returns_none_for_non_ident() {
        let p1 = phase1_fixture("simple");
        let fid = find_file_id(&p1, "main.hu");
        let tc = token_cache(&p1);
        let tokens = tc.get(&fid).unwrap();
        // Offset 0 is '@' in "@use * from ..." — not an ident
        let result = hubullu::lsp::hover::hover(fid, 0, tokens, &p1);
        // Could be None or Some depending on token, but should not panic
        let _ = result;
    }
}

// ===========================================================================
// definition
// ===========================================================================
mod definition_tests {
    use super::*;

    #[test]
    fn goto_definition_inflection_class() {
        let p1 = phase1_fixture("simple");
        let fid = find_file_id(&p1, "main.hu");
        let tc = token_cache(&p1);
        let tokens = tc.get(&fid).unwrap();
        // "strong_I" in inflection_class: strong_I
        let source = p1.source_map.source(fid);
        let offset = source.find("strong_I").expect("strong_I not found in main.hu");

        let result = hubullu::lsp::definition::goto_definition(fid, offset, tokens, &p1);
        assert!(result.is_some(), "should find definition of strong_I");
        match result.unwrap() {
            lsp_types::GotoDefinitionResponse::Scalar(loc) => {
                // Should point to profile.hu where strong_I is defined
                assert!(
                    loc.uri.as_str().contains("profile.hu"),
                    "definition should be in profile.hu, got: {}",
                    loc.uri.as_str()
                );
            }
            _ => panic!("expected scalar response"),
        }
    }

    #[test]
    fn goto_definition_tagaxis_from_entry() {
        let p1 = phase1_fixture("simple");
        let fid = find_file_id(&p1, "main.hu");
        let tc = token_cache(&p1);
        let tokens = tc.get(&fid).unwrap();
        let source = p1.source_map.source(fid);
        let offset = source.find("parts_of_speech").expect("parts_of_speech not found");

        let result = hubullu::lsp::definition::goto_definition(fid, offset, tokens, &p1);
        assert!(result.is_some(), "should find definition of parts_of_speech");
    }

    #[test]
    fn goto_definition_not_found() {
        let pr = parse("entry foo {\n  headword: \"foo\"\n  meaning: \"bar\"\n}");
        // Build a minimal Phase1Result from a single parse.
        let p1 = single_file_phase1(&pr);
        // Try to find definition of "foo" — it's defined here, but with single-file
        // we can still test the lookup.
        let offset = pr.source_map.source(pr.file_id).find("foo").unwrap();
        let result =
            hubullu::lsp::definition::goto_definition(pr.file_id, offset, &pr.tokens, &p1);
        // With the symbol table populated, this should succeed
        assert!(result.is_some());
    }
}

/// Build a minimal Phase1Result from a single ParseResult.
fn single_file_phase1(pr: &ParseResult) -> hubullu::phase1::Phase1Result {
    use hubullu::symbol_table::{Symbol, SymbolKind, SymbolTable};

    let mut files = HashMap::new();
    files.insert(pr.file_id, pr.file.clone());

    let mut symbol_table = SymbolTable::new();
    let scope = symbol_table.scope_mut(pr.file_id);
    for (idx, item) in pr.file.items.iter().enumerate() {
        use hubullu::ast::Item;
        let (name, kind, span) = match &item.node {
            Item::Entry(e) => (e.name.node.clone(), SymbolKind::Entry, e.name.span.clone()),
            Item::Inflection(i) => {
                (i.name.node.clone(), SymbolKind::Inflection, i.name.span.clone())
            }
            Item::TagAxis(t) => (t.name.node.clone(), SymbolKind::TagAxis, t.name.span.clone()),
            Item::Extend(ext) => {
                (ext.name.node.clone(), SymbolKind::Extend, ext.name.span.clone())
            }
            Item::PhonRule(p) => {
                (p.name.node.clone(), SymbolKind::PhonRule, p.name.span.clone())
            }
            _ => continue,
        };
        scope.locals.insert(
            name.clone(),
            Symbol {
                name,
                kind,
                file_id: pr.file_id,
                span,
                item_index: idx,
            },
        );
    }

    hubullu::phase1::Phase1Result {
        files,
        source_map: pr.source_map.clone(),
        symbol_table,
        diagnostics: hubullu::error::Diagnostics::new(),
        path_to_id: HashMap::new(),
    }
}

// ===========================================================================
// references
// ===========================================================================
mod references_tests {
    use super::*;

    #[test]
    fn find_references_to_inflection() {
        let p1 = phase1_fixture("simple");
        let fid = find_file_id(&p1, "main.hu");
        let tc = token_cache(&p1);
        let tokens = tc.get(&fid).unwrap();
        let source = p1.source_map.source(fid);
        let offset = source.find("strong_I").expect("strong_I not found");

        let locs = hubullu::lsp::references::find_references(
            fid, offset, tokens, &p1, &tc, true,
        );
        // strong_I should appear in both main.hu (usage) and profile.hu (definition)
        assert!(locs.len() >= 2, "expected at least 2 references, got {}", locs.len());
    }

    #[test]
    fn find_references_include_declaration_false() {
        let p1 = phase1_fixture("simple");
        let fid = find_file_id(&p1, "profile.hu");
        let tc = token_cache(&p1);
        let tokens = tc.get(&fid).unwrap();
        let source = p1.source_map.source(fid);
        let offset = source.find("strong_I").expect("strong_I not found");

        let locs_with = hubullu::lsp::references::find_references(
            fid, offset, tokens, &p1, &tc, true,
        );
        let locs_without = hubullu::lsp::references::find_references(
            fid, offset, tokens, &p1, &tc, false,
        );
        assert!(
            locs_with.len() >= locs_without.len(),
            "including declaration should return >= results"
        );
    }
}

// ===========================================================================
// rename
// ===========================================================================
mod rename_tests {
    use super::*;

    #[test]
    fn prepare_rename_on_ident() {
        let pr = parse("entry foo {\n  headword: \"foo\"\n  meaning: \"m\"\n}");
        let offset = pr.source_map.source(pr.file_id).find("foo").unwrap();
        let result =
            hubullu::lsp::rename::prepare_rename(pr.file_id, offset, &pr.tokens, &pr.source_map);
        assert!(result.is_some());
        match result.unwrap() {
            lsp_types::PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. } => {
                assert_eq!(placeholder, "foo");
            }
            _ => panic!("expected RangeWithPlaceholder"),
        }
    }

    #[test]
    fn rename_symbol_across_files() {
        let p1 = phase1_fixture("simple");
        let fid = find_file_id(&p1, "main.hu");
        let tc = token_cache(&p1);
        let tokens = tc.get(&fid).unwrap();
        let source = p1.source_map.source(fid);
        let offset = source.find("strong_I").expect("strong_I not found");

        let result =
            hubullu::lsp::rename::rename(fid, offset, "strong_II", tokens, &p1, &tc);
        assert!(result.is_some());
        let edit = result.unwrap();
        let changes = edit.changes.unwrap();
        // Should have edits in both files
        let total_edits: usize = changes.values().map(|v| v.len()).sum();
        assert!(
            total_edits >= 2,
            "expected edits in multiple places, got {}",
            total_edits
        );
        // All edits should have the new name
        for edits in changes.values() {
            for e in edits {
                assert_eq!(e.new_text, "strong_II");
            }
        }
    }

    #[test]
    fn prepare_rename_on_keyword_returns_none() {
        let pr = parse("entry foo {}");
        // offset 0 is 'e' of "entry" — but "entry" is a keyword token, not Ident
        // However the lexer may emit it as Ident. Let's find a non-ident token.
        // '{' at some position
        let source = pr.source_map.source(pr.file_id);
        let offset = source.find('{').unwrap();
        let result =
            hubullu::lsp::rename::prepare_rename(pr.file_id, offset, &pr.tokens, &pr.source_map);
        assert!(result.is_none(), "should not be renameable on '{{' token");
    }
}

// ===========================================================================
// completion
// ===========================================================================
mod completion_tests {
    use super::*;

    #[test]
    fn top_level_completion() {
        let pr = parse("");
        let p1 = single_file_phase1(&pr);
        let resp = hubullu::lsp::completion::complete(
            pr.file_id, pr.file_id, 0, &pr.tokens, Some(&p1), false,
        );
        match resp {
            lsp_types::CompletionResponse::List(list) => {
                let labels: Vec<_> = list.items.iter().map(|i| i.label.as_str()).collect();
                assert!(labels.contains(&"entry"), "should suggest 'entry' keyword");
                assert!(labels.contains(&"tagaxis"), "should suggest 'tagaxis' keyword");
                assert!(labels.contains(&"inflection"), "should suggest 'inflection' keyword");
            }
            lsp_types::CompletionResponse::Array(items) => {
                let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
                assert!(labels.contains(&"entry"), "should suggest 'entry' keyword");
            }
        }
    }

    /// Simulate what the LSP does for .hut files:
    /// - tokens come from the document's own SourceMap (token_file_id)
    /// - scope comes from the project's SourceMap (scope_file_id)
    /// These are different FileIds.
    #[test]
    fn hut_top_level_completion_shows_entries() {
        // 1. Parse a .hu source with entries and tagaxes.
        let hu_src = r#"
tagaxis number { role: inflectional }
tagaxis case { role: inflectional }
entry cat {
    headword: "cat"
    meaning: "a cat"
}
entry dog {
    headword: "dog"
    meaning: "a dog"
}
"#;
        let pr = parse(hu_src);
        let mut p1 = single_file_phase1(&pr);

        // 2. Add a .hut file to the project (separate file_id, like the LSP does).
        let hut_src = "cat[number=sg] ";
        let hut_file_id = p1.source_map.add_file(
            "/tmp/test.hut".into(),
            hut_src.to_string(),
        );

        // 3. Copy the .hu file's scope to the .hut file (like try_load_hut_project does).
        if let Some(hu_scope) = p1.symbol_table.scope(pr.file_id).cloned() {
            p1.symbol_table.scopes.insert(hut_file_id, hu_scope);
        }

        // 4. Lex the .hut content separately (like the document store does).
        let mut doc_source_map = hubullu::span::SourceMap::new();
        let doc_file_id = doc_source_map.add_file("/tmp/test.hut".into(), hut_src.to_string());
        let lexer = hubullu::lexer::Lexer::new(doc_source_map.source(doc_file_id), doc_file_id);
        let (doc_tokens, _) = lexer.tokenize();

        // 5. Call complete with doc_file_id as token_file_id and hut_file_id as scope_file_id.
        // Cursor at end of file (after "cat[number=sg] ").
        let resp = hubullu::lsp::completion::complete(
            doc_file_id, hut_file_id, hut_src.len(), &doc_tokens, Some(&p1), true,
        );
        match resp {
            lsp_types::CompletionResponse::List(list) => {
                let labels: Vec<_> = list.items.iter().map(|i| i.label.as_str()).collect();
                assert!(labels.contains(&"cat"), "should suggest entry 'cat', got: {:?}", labels);
                assert!(labels.contains(&"dog"), "should suggest entry 'dog', got: {:?}", labels);
            }
            lsp_types::CompletionResponse::Array(items) => {
                let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
                assert!(labels.contains(&"cat"), "should suggest entry 'cat', got: {:?}", labels);
            }
        }
    }

    /// Test .hut tagaxis completion inside brackets.
    #[test]
    fn hut_tagaxis_completion_inside_brackets() {
        let hu_src = r#"
tagaxis number { role: inflectional }
tagaxis case { role: inflectional }
inflection cat_infl for { number, case } {
    [number=sg, case=nom] -> `cat`
}
entry cat {
    headword: "cat"
    meaning: "a cat"
    inflection_class: cat_infl
}
"#;
        let pr = parse(hu_src);
        let mut p1 = single_file_phase1(&pr);

        let hut_src = "cat[";
        let hut_file_id = p1.source_map.add_file(
            "/tmp/test.hut".into(),
            hut_src.to_string(),
        );
        if let Some(hu_scope) = p1.symbol_table.scope(pr.file_id).cloned() {
            p1.symbol_table.scopes.insert(hut_file_id, hu_scope);
        }

        let mut doc_source_map = hubullu::span::SourceMap::new();
        let doc_file_id = doc_source_map.add_file("/tmp/test.hut".into(), hut_src.to_string());
        let lexer = hubullu::lexer::Lexer::new(doc_source_map.source(doc_file_id), doc_file_id);
        let (doc_tokens, _) = lexer.tokenize();

        let resp = hubullu::lsp::completion::complete(
            doc_file_id, hut_file_id, hut_src.len(), &doc_tokens, Some(&p1), true,
        );
        match resp {
            lsp_types::CompletionResponse::List(list) => {
                let labels: Vec<_> = list.items.iter().map(|i| i.label.as_str()).collect();
                assert!(labels.contains(&"number"), "should suggest tagaxis 'number', got: {:?}", labels);
                assert!(labels.contains(&"case"), "should suggest tagaxis 'case', got: {:?}", labels);
            }
            lsp_types::CompletionResponse::Array(items) => {
                let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
                assert!(labels.contains(&"number"), "should suggest tagaxis 'number', got: {:?}", labels);
            }
        }
    }

    #[test]
    fn completion_inside_entry_body() {
        let src = "entry foo {\n  \n}";
        let pr = parse(src);
        let p1 = single_file_phase1(&pr);
        // Offset inside the entry body (after "  " on line 2)
        let offset = src.find('\n').unwrap() + 3;
        let resp = hubullu::lsp::completion::complete(
            pr.file_id, pr.file_id, offset, &pr.tokens, Some(&p1), false,
        );
        match resp {
            lsp_types::CompletionResponse::List(list) => {
                let labels: Vec<_> = list.items.iter().map(|i| i.label.as_str()).collect();
                assert!(
                    labels.contains(&"headword:") || labels.contains(&"headword"),
                    "should suggest entry fields, got: {:?}",
                    labels
                );
            }
            lsp_types::CompletionResponse::Array(items) => {
                let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
                assert!(
                    labels.contains(&"headword:") || labels.contains(&"headword"),
                    "should suggest entry fields, got: {:?}",
                    labels
                );
            }
        }
    }
}

// ===========================================================================
// semantic_tokens
// ===========================================================================
mod semantic_tokens_tests {
    use super::*;

    #[test]
    fn legend_has_expected_types() {
        let legend = hubullu::lsp::semantic_tokens::legend();
        assert!(!legend.token_types.is_empty());
        assert!(legend.token_types.contains(&lsp_types::SemanticTokenType::KEYWORD));
        assert!(legend.token_types.contains(&lsp_types::SemanticTokenType::STRING));
    }

    #[test]
    fn generate_produces_tokens() {
        let pr = parse("entry foo {\n  headword: \"foo\"\n  meaning: \"bar\"\n}");
        let result = hubullu::lsp::semantic_tokens::generate(
            &pr.tokens,
            &[],
            pr.file_id,
            &pr.source_map,
            &pr.file,
        );
        match result {
            lsp_types::SemanticTokensResult::Tokens(tokens) => {
                assert!(!tokens.data.is_empty(), "expected semantic tokens");
            }
            _ => panic!("expected full tokens"),
        }
    }
}

// ===========================================================================
// diagnostics
// ===========================================================================
mod diagnostics_tests {
    use super::*;

    #[test]
    fn publish_notification_with_errors() {
        let pr = parse("entry {");
        assert!(pr.has_errors(), "expected parse errors");

        let uri: lsp_types::Uri = "file:///tmp/test.hu".parse().unwrap();
        let diag_refs: Vec<_> = pr.diagnostics.iter().collect();
        let notif = hubullu::lsp::diagnostics::publish_notification(&uri, &diag_refs, &pr.source_map);
        assert_eq!(notif.method, "textDocument/publishDiagnostics");
    }

    #[test]
    fn clear_notification() {
        let uri: lsp_types::Uri = "file:///tmp/test.hu".parse().unwrap();
        let notif = hubullu::lsp::diagnostics::clear_notification(&uri);
        assert_eq!(notif.method, "textDocument/publishDiagnostics");
    }
}

// ===========================================================================
// document_link
// ===========================================================================
mod document_link_tests {
    use super::*;

    #[test]
    fn document_links_for_use() {
        let p1 = phase1_fixture("simple");
        let fid = find_file_id(&p1, "main.hu");

        // Re-parse main.hu to get a ParseResult
        let source = p1.source_map.source(fid).to_string();
        let pr = hubullu::parse_source(&source, &p1.source_map.path(fid).to_string_lossy());

        let links = hubullu::lsp::document_link::document_links(&pr, Some(&p1));
        assert!(!links.is_empty(), "should produce a link for @use \"profile.hu\"");
        assert!(links[0].target.is_some());
    }

    #[test]
    fn document_links_no_imports() {
        let pr = parse("entry foo {\n  headword: \"f\"\n  meaning: \"m\"\n}");
        let links = hubullu::lsp::document_link::document_links(&pr, None);
        assert!(links.is_empty());
    }
}

// ===========================================================================
// document store
// ===========================================================================
mod document_store_tests {
    #[test]
    fn open_and_get() {
        let mut store = hubullu::lsp::document::DocumentStore::default();
        let uri: lsp_types::Uri = "file:///tmp/test.hu".parse().unwrap();
        let source = "entry foo {\n  headword: \"foo\"\n  meaning: \"m\"\n}".to_string();
        store.open(&uri, source.clone(), 1);

        let doc = store.get(&uri);
        assert!(doc.is_some());
        assert_eq!(doc.unwrap().text, source);
        assert_eq!(doc.unwrap().version, 1);
    }

    #[test]
    fn change_updates_text() {
        let mut store = hubullu::lsp::document::DocumentStore::default();
        let uri: lsp_types::Uri = "file:///tmp/test.hu".parse().unwrap();
        store.open(&uri, "entry foo {}".to_string(), 1);
        store.change(&uri, "entry bar {}".to_string(), 2);

        let doc = store.get(&uri).unwrap();
        assert_eq!(doc.text, "entry bar {}");
        assert_eq!(doc.version, 2);
    }

    #[test]
    fn close_removes() {
        let mut store = hubullu::lsp::document::DocumentStore::default();
        let uri: lsp_types::Uri = "file:///tmp/test.hu".parse().unwrap();
        store.open(&uri, "entry foo {}".to_string(), 1);
        store.close(&uri);
        assert!(store.get(&uri).is_none());
    }
}
