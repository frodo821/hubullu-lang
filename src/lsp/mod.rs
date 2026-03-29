//! Language Server Protocol implementation for hubullu.
//!
//! Provides diagnostics, semantic tokens, go-to-definition, hover, and completion
//! for `.hu` files.

mod completion;
mod convert;
mod definition;
mod diagnostics;
mod disk_cache;
mod document;
mod document_link;
mod folding;
mod formatting;
mod hut_source;
mod inlay_hints;
mod hover;
mod references;
mod rename;
mod semantic_tokens;
mod symbols;

use std::path::PathBuf;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    CompletionOptions, InitializeParams, OneOf, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensServerCapabilities, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, Uri, WorkDoneProgressOptions,
};

use crate::phase1::Phase1Result;
use crate::phase2::Phase2Result;
use crate::span::FileId;
use crate::token::Token;

use document::DocumentStore;

/// Cached project-level analysis (populated on save).
struct ProjectState {
    phase1: Phase1Result,
    /// Phase2 result (available when phase1 has no errors).
    phase2: Option<Phase2Result>,
    /// Per-file token cache (lexed once during project analysis).
    token_cache: std::collections::HashMap<FileId, Vec<Token>>,
    /// Map from URI string to FileId in the phase1 source map.
    url_to_file_id: std::collections::HashMap<String, FileId>,
}

/// Run the LSP server on stdin/stdout.
pub fn run_server() {
    let (connection, io_threads) = Connection::stdio();

    let server_caps = serde_json::to_value(server_capabilities()).unwrap();
    let init_params = connection.initialize(server_caps).unwrap();
    let init_params: InitializeParams = serde_json::from_value(init_params).unwrap();

    main_loop(&connection, init_params);

    // Drop the connection to flush the shutdown response to stdout and
    // close the channels, allowing IO threads to finish.
    drop(connection);

    // io_threads.join() can hang if the stdin reader blocks waiting for
    // the exit notification. Spawn a thread to attempt the join and give
    // it a brief window before exiting the process.
    std::thread::spawn(move || { let _ = io_threads.join(); });
    std::thread::sleep(std::time::Duration::from_millis(100));
    std::process::exit(0);
}

fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        definition_provider: Some(OneOf::Left(true)),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![
                "@".into(),
                ".".into(),
                ":".into(),
                "[".into(),
                "=".into(),
            ]),
            ..Default::default()
        }),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: semantic_tokens::legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: None,
                ..Default::default()
            },
        )),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        document_link_provider: Some(lsp_types::DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        }),
        folding_range_provider: Some(lsp_types::FoldingRangeProviderCapability::Simple(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        inlay_hint_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(lsp_types::RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        })),
        ..Default::default()
    }
}

/// Server-wide state.
struct ServerState {
    documents: DocumentStore,
    /// Main project (from workspace root).
    project: Option<ProjectState>,
    /// Per-.hu file projects (keyed by the entry file's URI string).
    /// Used when a .hu file is opened outside the main project.
    file_projects: std::collections::HashMap<String, ProjectState>,
    /// Per-.hut file projects (keyed by .hut URI string, from @source directive).
    hut_projects: std::collections::HashMap<String, ProjectState>,
    init_params: InitializeParams,
}

impl ServerState {
    /// Get the project state relevant for a given URI.
    ///
    /// Search order:
    /// 1. hut_projects (for .hut files with @source)
    /// 2. main project (if it covers this file)
    /// 3. file_projects (incremental per-file projects)
    fn project_for(&self, uri: &Uri) -> Option<&ProjectState> {
        // .hut-specific project
        if let Some(proj) = self.hut_projects.get(uri.as_str()) {
            return Some(proj);
        }
        // Main project (if it knows about this file)
        if let Some(ref proj) = self.project {
            if proj.url_to_file_id.contains_key(uri.as_str()) {
                return Some(proj);
            }
        }
        // Per-file incremental project (check if any covers this URI)
        for proj in self.file_projects.values() {
            if proj.url_to_file_id.contains_key(uri.as_str()) {
                return Some(proj);
            }
        }
        // Fallback: main project even if it doesn't cover this file
        // (still useful for workspace symbols, etc.)
        self.project.as_ref()
    }

    /// Ensure a project exists that covers the given .hu file.
    /// If none exists, run phase1 from this file as entry point.
    fn ensure_project_for_hu(&mut self, uri: &Uri) {
        if is_hut_uri(uri) {
            return;
        }
        // Already covered by main project?
        if let Some(ref proj) = self.project {
            if proj.url_to_file_id.contains_key(uri.as_str()) {
                return;
            }
        }
        // Already covered by a file project?
        for proj in self.file_projects.values() {
            if proj.url_to_file_id.contains_key(uri.as_str()) {
                return;
            }
        }
        // Build a new project from this file.
        if let Some(path) = convert::uri_to_path(uri) {
            if path.exists() {
                if let Some(proj) = build_project_state(&path) {
                    self.file_projects.insert(uri.as_str().to_string(), proj);
                }
            }
        }
    }

    /// Re-analyze the file project that covers this URI.
    fn refresh_file_project(&mut self, uri: &Uri) {
        if is_hut_uri(uri) {
            return;
        }
        // Find which file_project entry covers this URI.
        let entry_key = self
            .file_projects
            .iter()
            .find(|(_, proj)| proj.url_to_file_id.contains_key(uri.as_str()))
            .map(|(k, _)| k.clone());
        if let Some(key) = entry_key {
            if let Some(entry_uri) = key.parse::<Uri>().ok() {
                if let Some(path) = convert::uri_to_path(&entry_uri) {
                    if let Some(new_proj) = build_project_state(&path) {
                        self.file_projects.insert(key, new_proj);
                    }
                }
            }
        }
    }
}

fn main_loop(connection: &Connection, init_params: InitializeParams) {
    let mut state = ServerState {
        documents: DocumentStore::default(),
        project: None,
        file_projects: std::collections::HashMap::new(),
        hut_projects: std::collections::HashMap::new(),
        init_params,
    };

    // Try to discover and analyze the project on startup.
    if let Some(root) = workspace_root(&state.init_params) {
        state.project = try_analyze_project(&root);
    }

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req).unwrap() {
                    return;
                }
                let resp = handle_request(req, &state);
                connection.sender.send(Message::Response(resp)).unwrap();
            }
            Message::Notification(notif) => {
                let notifications = handle_notification(notif, &mut state);
                for n in notifications {
                    connection.sender.send(Message::Notification(n)).unwrap();
                }
            }
            Message::Response(_) => {}
        }
    }
}

fn handle_request(req: Request, state: &ServerState) -> Response {
    let id = req.id.clone();

    match req.method.as_str() {
        "textDocument/definition" => handle_definition(id, req, state),
        "textDocument/hover" => handle_hover(id, req, state),
        "textDocument/completion" => handle_completion(id, req, state),
        "textDocument/semanticTokens/full" => handle_semantic_tokens(id, req, state),
        "textDocument/documentSymbol" => handle_document_symbol(id, req, state),
        "workspace/symbol" => handle_workspace_symbol(id, req, state),
        "textDocument/references" => handle_references(id, req, state),
        "textDocument/documentLink" => handle_document_link(id, req, state),
        "textDocument/foldingRange" => handle_folding_range(id, req, state),
        "textDocument/documentHighlight" => handle_document_highlight(id, req, state),
        "textDocument/inlayHint" => handle_inlay_hint(id, req, state),
        "textDocument/formatting" => handle_formatting(id, req, state),
        "textDocument/prepareRename" => handle_prepare_rename(id, req, state),
        "textDocument/rename" => handle_rename(id, req, state),
        _ => Response::new_err(id, -32601, "method not found".into()),
    }
}

fn handle_notification(notif: Notification, state: &mut ServerState) -> Vec<Notification> {
    let mut out = Vec::new();

    match notif.method.as_str() {
        "textDocument/didOpen" => {
            let params: lsp_types::DidOpenTextDocumentParams =
                serde_json::from_value(notif.params).unwrap();
            let uri = params.text_document.uri.clone();
            let text = params.text_document.text.clone();
            state.documents.open(
                &params.text_document.uri,
                params.text_document.text,
                params.text_document.version,
            );
            if is_hut_uri(&uri) {
                try_load_hut_project(&uri, &text, &mut state.hut_projects);
            } else {
                // Incremental: if no project covers this .hu file, run phase1 from it.
                state.ensure_project_for_hu(&uri);
            }
            let project = state.project_for(&uri);
            out.push(publish_doc_diagnostics(&uri, &state.documents, project));
        }
        "textDocument/didChange" => {
            let params: lsp_types::DidChangeTextDocumentParams =
                serde_json::from_value(notif.params).unwrap();
            let uri = params.text_document.uri.clone();
            if let Some(change) = params.content_changes.into_iter().last() {
                // For .hut files, re-check @source on change.
                if is_hut_uri(&uri) {
                    try_load_hut_project(&uri, &change.text, &mut state.hut_projects);
                }
                state.documents.change(&uri, change.text, params.text_document.version);
            }
            let project = state.project_for(&uri);
            out.push(publish_doc_diagnostics(&uri, &state.documents, project));
        }
        "textDocument/didSave" => {
            let params: lsp_types::DidSaveTextDocumentParams =
                serde_json::from_value(notif.params).unwrap();
            let uri = params.text_document.uri.clone();

            // Re-run main project analysis on save.
            if let Some(root) = workspace_root(&state.init_params) {
                state.project = try_analyze_project(&root);
            }

            if is_hut_uri(&uri) {
                if let Some(doc) = state.documents.get(&uri) {
                    let text = doc.text.clone();
                    try_load_hut_project(&uri, &text, &mut state.hut_projects);
                }
            } else {
                // Refresh file-level project on save.
                state.refresh_file_project(&uri);
                // If still not covered, create one.
                state.ensure_project_for_hu(&uri);
            }

            let project = state.project_for(&uri);
            out.push(publish_doc_diagnostics(&uri, &state.documents, project));
        }
        "textDocument/didClose" => {
            let params: lsp_types::DidCloseTextDocumentParams =
                serde_json::from_value(notif.params).unwrap();
            let uri_str = params.text_document.uri.as_str();
            state.documents.close(&params.text_document.uri);
            state.hut_projects.remove(uri_str);
            state.file_projects.remove(uri_str);
            out.push(diagnostics::clear_notification(&params.text_document.uri));
        }
        _ => {}
    }

    out
}

// ---------------------------------------------------------------------------
// Request handlers
// ---------------------------------------------------------------------------

fn handle_definition(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::GotoDefinitionParams = serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document_position_params.text_document.uri;
    let project = s.project_for(uri);
    let result = (|| {
        let proj = project?;
        let file_id = find_file_id(uri, proj)?;
        let offset = convert::position_to_offset(
            &params.text_document_position_params.position, file_id, &proj.phase1.source_map,
        )?;
        let tokens = proj.token_cache.get(&file_id)?;
        definition::goto_definition(file_id, offset, tokens, &proj.phase1)
    })();
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_hover(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::HoverParams = serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document_position_params.text_document.uri;
    let project = s.project_for(uri);
    let result = (|| {
        let proj = project?;
        let file_id = find_file_id(uri, proj)?;
        let offset = convert::position_to_offset(
            &params.text_document_position_params.position, file_id, &proj.phase1.source_map,
        )?;
        let tokens = proj.token_cache.get(&file_id)?;
        hover::hover(file_id, offset, tokens, &proj.phase1)
    })();
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_completion(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::CompletionParams = serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document_position.text_document.uri;
    let project = s.project_for(uri);
    let result = (|| {
        let doc = s.documents.get(uri)?;
        let scope_file_id = project
            .and_then(|p| find_file_id(uri, p))
            .unwrap_or(doc.parse_result.file_id);
        let offset = convert::position_to_offset(
            &params.text_document_position.position, doc.parse_result.file_id, &doc.parse_result.source_map,
        )?;
        Some(completion::complete(
            doc.parse_result.file_id, scope_file_id, offset, &doc.parse_result.tokens,
            project.map(|p| &p.phase1),
            is_hut_uri(uri),
        ))
    })();
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_semantic_tokens(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::SemanticTokensParams = serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document.uri;
    let result = s.documents.get(uri).map(|doc| {
        semantic_tokens::generate(
            &doc.parse_result.tokens, &[],
            doc.parse_result.file_id, &doc.parse_result.source_map,
            &doc.parse_result.file,
        )
    });
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_document_symbol(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::DocumentSymbolParams = serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document.uri;
    let result = s.documents.get(uri).map(|doc| {
        lsp_types::DocumentSymbolResponse::Nested(symbols::document_symbols(&doc.parse_result))
    });
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_workspace_symbol(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::WorkspaceSymbolParams = serde_json::from_value(req.params).unwrap();
    let result = s.project.as_ref().map(|proj| {
        symbols::workspace_symbols(&params.query, &proj.phase1)
    });
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_references(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::ReferenceParams = serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document_position.text_document.uri;
    let project = s.project_for(uri);
    let result: Option<Vec<lsp_types::Location>> = (|| {
        let proj = project?;
        let file_id = find_file_id(uri, proj)?;
        let offset = convert::position_to_offset(
            &params.text_document_position.position, file_id, &proj.phase1.source_map,
        )?;
        let tokens = proj.token_cache.get(&file_id)?;
        Some(references::find_references(
            file_id, offset, tokens, &proj.phase1,
            &proj.token_cache, params.context.include_declaration,
        ))
    })();
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_document_link(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::DocumentLinkParams = serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document.uri;
    let project = s.project_for(uri);
    let result = s.documents.get(uri).map(|doc| {
        document_link::document_links(&doc.parse_result, project.map(|p| &p.phase1))
    });
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_folding_range(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::FoldingRangeParams = serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document.uri;
    let result = s.documents.get(uri).map(|doc| folding::folding_ranges(&doc.parse_result));
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_document_highlight(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::DocumentHighlightParams = serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document_position_params.text_document.uri;
    let result: Option<Vec<lsp_types::DocumentHighlight>> = (|| {
        let doc = s.documents.get(uri)?;
        let file_id = doc.parse_result.file_id;
        let source_map = &doc.parse_result.source_map;
        let offset = convert::position_to_offset(
            &params.text_document_position_params.position, file_id, source_map,
        )?;
        let target_name = doc.parse_result.tokens.iter().find_map(|t| {
            if t.span.file_id == file_id && t.span.start <= offset && offset < t.span.end {
                if let crate::token::TokenKind::Ident(name) = &t.node {
                    return Some(name.clone());
                }
            }
            None
        })?;
        let highlights: Vec<_> = doc.parse_result.tokens.iter().filter_map(|t| {
            if t.span.file_id == file_id {
                if let crate::token::TokenKind::Ident(name) = &t.node {
                    if name == &target_name {
                        return Some(lsp_types::DocumentHighlight {
                            range: convert::span_to_range(&t.span, source_map),
                            kind: Some(lsp_types::DocumentHighlightKind::TEXT),
                        });
                    }
                }
            }
            None
        }).collect();
        Some(highlights)
    })();
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_inlay_hint(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::InlayHintParams = serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document.uri;
    let project = s.project_for(uri);
    let result: Option<Vec<lsp_types::InlayHint>> = (|| {
        let proj = project?;
        let p2 = proj.phase2.as_ref()?;
        let file_id = find_file_id(uri, proj)?;
        if is_hut_uri(uri) {
            // .hut files: extract entry refs from the token stream.
            let tokens = proj.token_cache.get(&file_id)?;
            Some(inlay_hints::inlay_hints_from_tokens(
                file_id, tokens, p2, &proj.phase1.source_map,
            ))
        } else {
            let file_ast = proj.phase1.files.get(&file_id)?;
            Some(inlay_hints::inlay_hints(file_id, file_ast, p2, &proj.phase1.source_map))
        }
    })();
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_formatting(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::DocumentFormattingParams = serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document.uri;
    let result = s.documents.get(uri).map(|doc| formatting::format_document(&doc.text));
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_prepare_rename(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::TextDocumentPositionParams = serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document.uri;
    let result = (|| {
        let doc = s.documents.get(uri)?;
        let offset = convert::position_to_offset(
            &params.position, doc.parse_result.file_id, &doc.parse_result.source_map,
        )?;
        rename::prepare_rename(
            doc.parse_result.file_id, offset, &doc.parse_result.tokens, &doc.parse_result.source_map,
        )
    })();
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_rename(id: RequestId, req: Request, s: &ServerState) -> Response {
    let params: lsp_types::RenameParams = serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document_position.text_document.uri;
    let project = s.project_for(uri);
    let result = (|| {
        let proj = project?;
        let file_id = find_file_id(uri, proj)?;
        let offset = convert::position_to_offset(
            &params.text_document_position.position, file_id, &proj.phase1.source_map,
        )?;
        let tokens = proj.token_cache.get(&file_id)?;
        rename::rename(
            file_id, offset, &params.new_name, tokens,
            &proj.phase1, &proj.token_cache,
        )
    })();
    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn publish_doc_diagnostics(
    uri: &Uri,
    documents: &DocumentStore,
    project: Option<&ProjectState>,
) -> Notification {
    let doc = match documents.get(uri) {
        Some(d) => d,
        None => return diagnostics::clear_notification(uri),
    };

    // Start with single-file parse diagnostics.
    let mut diags: Vec<&crate::error::Diagnostic> =
        doc.parse_result.diagnostics.iter().collect();

    // If project analysis is available, include phase1 diagnostics for this file.
    if let Some(proj) = project {
        if let Some(&fid) = proj.url_to_file_id.get(uri.as_str()) {
            let p1_diags: Vec<_> = proj
                .phase1
                .diagnostics
                .errors
                .iter()
                .filter(|d| d.labels.first().is_some_and(|l| l.span.file_id == fid))
                .collect();
            diags.extend(p1_diags);
        }
    }

    diagnostics::publish_notification(uri, &diags, &doc.parse_result.source_map)
}

fn workspace_root(init_params: &InitializeParams) -> Option<PathBuf> {
    // Try workspace_folders first, fall back to deprecated root_uri.
    if let Some(ref folders) = init_params.workspace_folders {
        if let Some(folder) = folders.first() {
            return convert::uri_to_path(&folder.uri);
        }
    }
    #[allow(deprecated)]
    init_params
        .root_uri
        .as_ref()
        .and_then(|uri| convert::uri_to_path(uri))
}

fn try_analyze_project(root: &PathBuf) -> Option<ProjectState> {
    let entry_path = find_entry_file(root)?;
    build_project_state(&entry_path)
}

/// Build a complete ProjectState from an entry file path.
/// Tries disk cache first; if stale or missing, runs full analysis and saves.
fn build_project_state(entry_path: &std::path::Path) -> Option<ProjectState> {
    // Try disk cache first.
    if let Some(cached) = disk_cache::load(entry_path) {
        let url_to_file_id = build_url_map(&cached.phase1.source_map);
        return Some(ProjectState {
            phase1: cached.phase1,
            phase2: cached.phase2,
            token_cache: cached.token_cache,
            url_to_file_id,
        });
    }

    // Cache miss — run full analysis.
    let phase1 = crate::phase1::run_phase1(entry_path);

    let phase2 = if !phase1.diagnostics.has_errors() {
        let p2 = crate::phase2::run_phase2(&phase1);
        if !p2.diagnostics.has_errors() { Some(p2) } else { None }
    } else {
        None
    };

    let mut token_cache = std::collections::HashMap::new();
    for fid in phase1.source_map.file_ids() {
        let source = phase1.source_map.source(fid);
        let lexer = crate::lexer::Lexer::new(source, fid);
        let (tokens, _) = lexer.tokenize();
        token_cache.insert(fid, tokens);
    }

    // Save to disk cache for next startup.
    disk_cache::save(entry_path, &phase1, phase2.as_ref(), &token_cache);

    let url_to_file_id = build_url_map(&phase1.source_map);
    Some(ProjectState { phase1, phase2, token_cache, url_to_file_id })
}

fn build_url_map(source_map: &crate::span::SourceMap) -> std::collections::HashMap<String, FileId> {
    let mut map = std::collections::HashMap::new();
    for fid in source_map.file_ids() {
        let path = source_map.path(fid);
        if let Some(uri) = convert::path_to_uri(path) {
            map.insert(uri.as_str().to_string(), fid);
        }
    }
    map
}

fn find_entry_file(root: &PathBuf) -> Option<PathBuf> {
    let main_hu = root.join("main.hu");
    if main_hu.exists() {
        return Some(main_hu);
    }

    let hu_files: Vec<_> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "hu"))
        .collect();

    if hu_files.len() == 1 {
        Some(hu_files[0].path())
    } else {
        None
    }
}

fn find_file_id(uri: &Uri, project: &ProjectState) -> Option<FileId> {
    project.url_to_file_id.get(uri.as_str()).copied()
}

pub(super) fn is_hut_uri(uri: &Uri) -> bool {
    uri.as_str().ends_with(".hut")
}

/// Load a project for a `.hut` file based on its `# @source` directive.
///
/// After building the project from the `.hu` entry, the `.hut` file itself is
/// injected into the project's source map, token cache, symbol table, and file
/// map so that LSP features (hover, go-to-definition, references, inlay hints)
/// work seamlessly.
fn try_load_hut_project(
    hut_uri: &Uri,
    hut_text: &str,
    hut_projects: &mut std::collections::HashMap<String, ProjectState>,
) {
    let source_path = match hut_source::parse_source_directive(hut_text) {
        Some(p) => p,
        None => return,
    };
    let hut_file_path = match convert::uri_to_path(hut_uri) {
        Some(p) => p,
        None => return,
    };
    let entry_path = hut_source::resolve_source_path(&hut_file_path, &source_path);
    if !entry_path.exists() {
        return;
    }
    if let Some(mut proj) = build_project_state(&entry_path) {
        // Add the .hut file to the project so LSP features work.
        let hut_filename = convert::uri_to_filename(hut_uri);
        let hut_file_id = proj.phase1.source_map.add_file(
            hut_filename.into(),
            hut_text.to_string(),
        );
        proj.url_to_file_id
            .insert(hut_uri.as_str().to_string(), hut_file_id);

        // Tokenize the .hut file and add to token cache.
        let lexer = crate::lexer::Lexer::new(
            proj.phase1.source_map.source(hut_file_id),
            hut_file_id,
        );
        let (tokens, _) = lexer.tokenize();
        proj.token_cache.insert(hut_file_id, tokens);

        // Create a scope for the .hut file by mirroring the entry file's scope
        // so that symbol resolution works.
        if let Some(&entry_fid) = proj.phase1.path_to_id.get(&entry_path) {
            if let Some(entry_scope) = proj.phase1.symbol_table.scope(entry_fid).cloned() {
                let mut hut_scope = crate::symbol_table::Scope::new();
                // Import the entry file's locals as imports for the .hut scope.
                for sym in entry_scope.locals.values() {
                    hut_scope.imports.push(crate::symbol_table::ImportedSymbol {
                        original_name: sym.name.clone(),
                        local_name: sym.name.clone(),
                        namespace: None,
                        kind: sym.kind,
                        source_file: sym.file_id,
                        span: sym.span,
                        item_index: sym.item_index,
                    });
                }
                // Carry over the entry file's imports.
                hut_scope.imports.extend(entry_scope.imports);
                proj.phase1.symbol_table.scopes.insert(hut_file_id, hut_scope);
            }
        }

        // Register an empty AST for the .hut file_id.
        proj.phase1.files.insert(
            hut_file_id,
            crate::ast::File { items: Vec::new() },
        );

        hut_projects.insert(hut_uri.as_str().to_string(), proj);
    }
}
