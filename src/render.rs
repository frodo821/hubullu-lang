//! `.hut` file rendering — resolves token lists against a compiled SQLite database.

use rusqlite::Connection;

use crate::ast;
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::span::SourceMap;

/// Parse a `.hut` source string into a token list.
pub fn parse_hut(source: &str, filename: &str) -> Result<Vec<ast::Token>, String> {
    let mut source_map = SourceMap::new();
    let file_id = source_map.add_file(filename.into(), source.to_string());

    let lexer = Lexer::new(source_map.source(file_id), file_id);
    let (tokens, lex_errors) = lexer.tokenize();
    if !lex_errors.is_empty() {
        let msgs: Vec<String> = lex_errors.iter().map(|e| e.render(&source_map)).collect();
        return Err(msgs.join("\n"));
    }

    let parser = Parser::new(tokens, file_id);
    let (ast_tokens, parse_errors) = parser.parse_token_list_to_eof();
    if !parse_errors.is_empty() {
        let msgs: Vec<String> = parse_errors.iter().map(|e| e.render(&source_map)).collect();
        return Err(msgs.join("\n"));
    }

    Ok(ast_tokens)
}

/// Resolve a list of AST tokens into string parts using the database.
pub fn resolve(tokens: &[ast::Token], db: &Connection) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    for token in tokens {
        match token {
            ast::Token::Lit(s) => {
                parts.push(s.node.clone());
            }
            ast::Token::Ref(entry_ref) => {
                let entry_id = &entry_ref.entry_id.node;
                match &entry_ref.form_spec {
                    None => {
                        // Look up headword
                        let headword: String = db
                            .query_row(
                                "SELECT headword FROM entries WHERE entry_id = ?1",
                                [entry_id],
                                |row| row.get(0),
                            )
                            .map_err(|e| format!("entry '{}' not found: {}", entry_id, e))?;
                        parts.push(headword);
                    }
                    Some(form_spec) => {
                        // Build requested tags as a set for matching
                        let mut requested: Vec<(String, String)> = form_spec
                            .conditions
                            .iter()
                            .map(|c| (c.axis.node.clone(), c.value.node.clone()))
                            .collect();
                        requested.sort();

                        // Query all forms for this entry and find the matching one
                        let mut stmt = db
                            .prepare("SELECT form_str, tags FROM forms WHERE entry_id = ?1")
                            .map_err(|e| format!("query failed: {}", e))?;
                        let mut rows = stmt
                            .query([entry_id])
                            .map_err(|e| format!("query failed: {}", e))?;

                        let mut found = None;
                        while let Some(row) = rows
                            .next()
                            .map_err(|e| format!("query failed: {}", e))?
                        {
                            let form_str: String =
                                row.get(0).map_err(|e| format!("read failed: {}", e))?;
                            let tags_str: String =
                                row.get(1).map_err(|e| format!("read failed: {}", e))?;
                            let mut stored: Vec<(String, String)> = tags_str
                                .split(',')
                                .filter(|s| !s.is_empty())
                                .filter_map(|pair| {
                                    let mut parts = pair.splitn(2, '=');
                                    Some((parts.next()?.to_string(), parts.next()?.to_string()))
                                })
                                .collect();
                            stored.sort();
                            if stored == requested {
                                found = Some(form_str);
                                break;
                            }
                        }

                        let form_str = found.ok_or_else(|| {
                            let tags_display = form_spec
                                .conditions
                                .iter()
                                .map(|c| format!("{}={}", c.axis.node, c.value.node))
                                .collect::<Vec<_>>()
                                .join(", ");
                            format!("form '{}[{}]' not found", entry_id, tags_display)
                        })?;
                        parts.push(form_str);
                    }
                }
            }
        }
    }
    Ok(parts)
}

/// Join string parts using separator, suppressing it before certain characters.
pub fn smart_join(parts: &[String], separator: &str, no_sep_before: &str) -> String {
    let mut result = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 && !separator.is_empty() {
            let first_char = part.chars().next();
            let suppress = first_char
                .map(|c| no_sep_before.contains(c))
                .unwrap_or(false);
            if !suppress {
                result.push_str(separator);
            }
        }
        result.push_str(part);
    }
    result
}

/// Read render config from the database, falling back to defaults.
pub fn read_render_config(db: &Connection) -> (String, String) {
    let separator = db
        .query_row(
            "SELECT value FROM render_config WHERE key = 'separator'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_else(|_| " ".to_string());

    let no_separator_before = db
        .query_row(
            "SELECT value FROM render_config WHERE key = 'no_separator_before'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_else(|_| ".,;:!?".to_string());

    (separator, no_separator_before)
}
