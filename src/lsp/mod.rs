//! Language Server Protocol implementation for hubullu.
//!
//! Provides diagnostics, semantic tokens, go-to-definition, hover, and completion
//! for `.hu` files.

mod completion;
mod convert;
mod definition;
mod diagnostics;
mod document;
mod hover;
mod semantic_tokens;

use std::path::PathBuf;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    CompletionOptions, InitializeParams, OneOf, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensServerCapabilities, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};

use crate::phase1::Phase1Result;
use crate::span::FileId;

use document::DocumentStore;

/// Cached project-level analysis (populated on save).
struct ProjectState {
    phase1: Phase1Result,
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

    io_threads.join().unwrap();
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
        ..Default::default()
    }
}

fn main_loop(connection: &Connection, init_params: InitializeParams) {
    let mut documents = DocumentStore::default();
    let mut project: Option<ProjectState> = None;

    // Try to discover and analyze the project on startup.
    if let Some(root) = workspace_root(&init_params) {
        project = try_analyze_project(&root);
    }

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req).unwrap() {
                    return;
                }
                let resp = handle_request(req, &documents, project.as_ref());
                connection.sender.send(Message::Response(resp)).unwrap();
            }
            Message::Notification(notif) => {
                let notifications =
                    handle_notification(notif, &mut documents, &mut project, &init_params);
                for n in notifications {
                    connection.sender.send(Message::Notification(n)).unwrap();
                }
            }
            Message::Response(_) => {}
        }
    }
}

fn handle_request(
    req: Request,
    documents: &DocumentStore,
    project: Option<&ProjectState>,
) -> Response {
    let id = req.id.clone();

    match req.method.as_str() {
        "textDocument/definition" => handle_definition(id, req, documents, project),
        "textDocument/hover" => handle_hover(id, req, documents, project),
        "textDocument/completion" => handle_completion(id, req, documents, project),
        "textDocument/semanticTokens/full" => handle_semantic_tokens(id, req, documents),
        _ => Response::new_err(id, -32601, "method not found".into()),
    }
}

fn handle_notification(
    notif: Notification,
    documents: &mut DocumentStore,
    project: &mut Option<ProjectState>,
    init_params: &InitializeParams,
) -> Vec<Notification> {
    let mut out = Vec::new();

    match notif.method.as_str() {
        "textDocument/didOpen" => {
            let params: lsp_types::DidOpenTextDocumentParams =
                serde_json::from_value(notif.params).unwrap();
            let uri = params.text_document.uri.clone();
            documents.open(
                &params.text_document.uri,
                params.text_document.text,
                params.text_document.version,
            );
            out.push(publish_doc_diagnostics(&uri, documents, project.as_ref()));
        }
        "textDocument/didChange" => {
            let params: lsp_types::DidChangeTextDocumentParams =
                serde_json::from_value(notif.params).unwrap();
            let uri = params.text_document.uri.clone();
            if let Some(change) = params.content_changes.into_iter().last() {
                documents.change(&uri, change.text, params.text_document.version);
            }
            out.push(publish_doc_diagnostics(&uri, documents, project.as_ref()));
        }
        "textDocument/didSave" => {
            if let Some(root) = workspace_root(init_params) {
                *project = try_analyze_project(&root);
            }
            let params: lsp_types::DidSaveTextDocumentParams =
                serde_json::from_value(notif.params).unwrap();
            out.push(publish_doc_diagnostics(
                &params.text_document.uri,
                documents,
                project.as_ref(),
            ));
        }
        "textDocument/didClose" => {
            let params: lsp_types::DidCloseTextDocumentParams =
                serde_json::from_value(notif.params).unwrap();
            documents.close(&params.text_document.uri);
            out.push(diagnostics::clear_notification(&params.text_document.uri));
        }
        _ => {}
    }

    out
}

// ---------------------------------------------------------------------------
// Request handlers
// ---------------------------------------------------------------------------

fn handle_definition(
    id: RequestId,
    req: Request,
    documents: &DocumentStore,
    project: Option<&ProjectState>,
) -> Response {
    let params: lsp_types::GotoDefinitionParams =
        serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document_position_params.text_document.uri;

    let result = (|| {
        let p1 = &project?.phase1;
        let doc = documents.get(uri)?;
        let file_id = find_file_id(uri, project?)?;
        let offset = convert::position_to_offset(
            &params.text_document_position_params.position,
            file_id,
            &p1.source_map,
        )?;
        definition::goto_definition(file_id, offset, &doc.parse_result.tokens, p1)
    })();

    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_hover(
    id: RequestId,
    req: Request,
    documents: &DocumentStore,
    project: Option<&ProjectState>,
) -> Response {
    let params: lsp_types::HoverParams =
        serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document_position_params.text_document.uri;

    let result = (|| {
        let p1 = &project?.phase1;
        let doc = documents.get(uri)?;
        let file_id = find_file_id(uri, project?)?;
        let offset = convert::position_to_offset(
            &params.text_document_position_params.position,
            file_id,
            &p1.source_map,
        )?;
        hover::hover(file_id, offset, &doc.parse_result.tokens, p1)
    })();

    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_completion(
    id: RequestId,
    req: Request,
    documents: &DocumentStore,
    project: Option<&ProjectState>,
) -> Response {
    let params: lsp_types::CompletionParams =
        serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document_position.text_document.uri;

    let result = (|| {
        let doc = documents.get(uri)?;
        let file_id = project.and_then(|p| find_file_id(uri, p));
        let offset = convert::position_to_offset(
            &params.text_document_position.position,
            doc.parse_result.file_id,
            &doc.parse_result.source_map,
        )?;
        Some(completion::complete(
            file_id.unwrap_or(doc.parse_result.file_id),
            offset,
            &doc.parse_result.tokens,
            project.map(|p| &p.phase1),
        ))
    })();

    Response::new_ok(id, serde_json::to_value(result).unwrap())
}

fn handle_semantic_tokens(
    id: RequestId,
    req: Request,
    documents: &DocumentStore,
) -> Response {
    let params: lsp_types::SemanticTokensParams =
        serde_json::from_value(req.params).unwrap();
    let uri = &params.text_document.uri;

    let result = documents.get(uri).map(|doc| {
        semantic_tokens::generate(
            &doc.parse_result.tokens,
            &[], // TODO: comment spans from lexer
            doc.parse_result.file_id,
            &doc.parse_result.source_map,
        )
    });

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
    let phase1 = crate::phase1::run_phase1(&entry_path);

    let mut url_to_file_id = std::collections::HashMap::new();
    for fid in phase1.source_map.file_ids() {
        let path = phase1.source_map.path(fid);
        if let Some(uri) = convert::path_to_uri(path) {
            url_to_file_id.insert(uri.as_str().to_string(), fid);
        }
    }

    Some(ProjectState {
        phase1,
        url_to_file_id,
    })
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
