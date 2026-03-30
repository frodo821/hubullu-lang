//! Language Server Protocol implementation for hubullu.
//!
//! Provides diagnostics, semantic tokens, go-to-definition, hover, and completion
//! for `.hu` files.

pub mod completion;
pub mod convert;
pub mod definition;
pub mod diagnostics;
mod disk_cache;
pub mod document;
pub mod document_link;
pub mod folding;
pub mod formatting;
pub mod inlay_hints;
pub mod hover;
pub mod surface_forms;
pub mod references;
pub mod rename;
pub mod semantic_tokens;
pub mod symbols;

use std::path::PathBuf;

use crossbeam_channel::{Receiver, bounded, never};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    CompletionOptions, InitializeParams, OneOf, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensServerCapabilities, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, Uri, WorkDoneProgressOptions,
};

use crate::lint::LintDiagnostic;
use crate::phase1::Phase1Result;
use crate::phase2::Phase2Result;
use crate::span::FileId;
use crate::token::Token;

use document::DocumentStore;

// ---------------------------------------------------------------------------
// Graceful shutdown triggers
// ---------------------------------------------------------------------------

/// Returns a channel that receives a message when SIGINT is delivered.
#[cfg(unix)]
fn sigint_channel() -> Receiver<()> {
    use std::sync::atomic::{AtomicBool, Ordering};

    static SIGINT_FIRED: AtomicBool = AtomicBool::new(false);

    // Install a minimal signal handler that sets the flag.
    unsafe {
        extern "C" fn handler(_sig: i32) {
            SIGINT_FIRED.store(true, Ordering::SeqCst);
        }
        // sigaction with SA_RESTART so we don't break other syscalls.
        let mut sa: libc_sigaction = std::mem::zeroed();
        sa.sa_handler = handler as *const () as usize;
        sa.sa_flags = 0x02; // SA_RESTART
        sigaction(2 /* SIGINT */, &sa, std::ptr::null_mut());
    }

    let (tx, rx) = bounded(1);
    std::thread::spawn(move || {
        loop {
            if SIGINT_FIRED.load(Ordering::SeqCst) {
                let _ = tx.send(());
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
    rx
}

/// Returns a channel that receives a message when SIGINT (Ctrl+C) is delivered.
#[cfg(windows)]
fn sigint_channel() -> Receiver<()> {
    use std::sync::atomic::{AtomicBool, Ordering};

    static SIGINT_FIRED: AtomicBool = AtomicBool::new(false);

    extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }

    extern "system" fn handler(_ctrl_type: u32) -> i32 {
        SIGINT_FIRED.store(true, std::sync::atomic::Ordering::SeqCst);
        1 // TRUE — signal handled
    }

    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }

    let (tx, rx) = bounded(1);
    std::thread::spawn(move || {
        loop {
            if SIGINT_FIRED.load(Ordering::SeqCst) {
                let _ = tx.send(());
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
    rx
}

// Minimal libc FFI for signal handling (avoids libc crate dependency).
#[cfg(target_os = "macos")]
#[repr(C)]
struct libc_sigaction {
    sa_handler: usize,
    sa_mask: u32,
    sa_flags: i32,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct libc_sigaction {
    sa_handler: usize,
    sa_flags: std::os::raw::c_ulong,
    sa_restorer: usize,
    sa_mask: [std::os::raw::c_ulong; 2],
}

#[cfg(unix)]
extern "C" {
    fn sigaction(sig: i32, act: *const libc_sigaction, oact: *mut libc_sigaction) -> i32;
}

/// Returns a channel that fires when the LSP client process exits.
///
/// Uses `process_id` from `InitializeParams` (the LSP client PID).  Falls back
/// to the OS parent PID on Unix when the client does not supply one.
fn client_exit_channel(client_pid: Option<u32>) -> Receiver<()> {
    let pid = match client_pid {
        Some(p) if p > 0 => p,
        _ => {
            #[cfg(unix)]
            {
                extern "C" { fn getppid() -> i32; }
                unsafe { getppid() as u32 }
            }
            #[cfg(windows)]
            {
                // No reliable fallback on Windows; return a channel that never
                // fires.  In practice every LSP client sends process_id.
                return never();
            }
        }
    };

    client_exit_channel_for_pid(pid)
}

#[cfg(unix)]
fn client_exit_channel_for_pid(pid: u32) -> Receiver<()> {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    let (tx, rx) = bounded(1);
    std::thread::spawn(move || {
        loop {
            // kill(pid, 0) checks if the process exists without sending a signal.
            let alive = unsafe { kill(pid as i32, 0) } == 0;
            if !alive {
                let _ = tx.send(());
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
    rx
}

#[cfg(windows)]
fn client_exit_channel_for_pid(pid: u32) -> Receiver<()> {
    extern "system" {
        fn OpenProcess(
            desired_access: u32,
            inherit_handles: i32,
            process_id: u32,
        ) -> *mut std::ffi::c_void;
        fn WaitForSingleObject(handle: *mut std::ffi::c_void, millis: u32) -> u32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }

    const SYNCHRONIZE: u32 = 0x0010_0000;
    const WAIT_OBJECT_0: u32 = 0;

    let (tx, rx) = bounded(1);
    std::thread::spawn(move || {
        let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            // Cannot open the process — assume it already exited.
            let _ = tx.send(());
            return;
        }
        // Block until the process exits (poll every 500 ms so the thread can
        // be joined on server shutdown without waiting forever).
        loop {
            let result = unsafe { WaitForSingleObject(handle, 500) };
            if result == WAIT_OBJECT_0 {
                unsafe { CloseHandle(handle); }
                let _ = tx.send(());
                return;
            }
        }
    });
    rx
}

// ---------------------------------------------------------------------------
// File watcher (notify crate – cross-platform: macOS / Linux / Windows)
// ---------------------------------------------------------------------------

/// Spawn a background file watcher on `root` that monitors `.hu` / `.hut` files.
///
/// Returns a [`Receiver`] that fires (at most once per debounce window) whenever
/// a relevant file is created, modified, or removed.  The watcher itself is
/// returned as well so that it is not dropped (which would stop watching).
fn file_watcher_channel(
    root: &std::path::Path,
) -> Option<(Receiver<()>, notify::RecommendedWatcher)> {
    use notify::{RecursiveMode, Watcher};

    let (raw_tx, raw_rx) = crossbeam_channel::unbounded::<()>();

    let mut watcher = notify::RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let dominated = event.paths.iter().any(|p| {
                    p.extension()
                        .is_some_and(|e| e == "hu" || e == "hut")
                });
                if dominated {
                    let _ = raw_tx.send(());
                }
            }
        },
        notify::Config::default(),
    )
    .ok()?;

    watcher.watch(root, RecursiveMode::Recursive).ok()?;

    // Debounce thread: collapses rapid-fire events into a single signal.
    let (tx, rx) = bounded::<()>(1);
    std::thread::spawn(move || {
        let debounce = std::time::Duration::from_millis(300);
        loop {
            // Block until the first event arrives.
            if raw_rx.recv().is_err() {
                return;
            }
            // Drain subsequent events within the debounce window.
            loop {
                match raw_rx.recv_timeout(debounce) {
                    Ok(()) => continue,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                }
            }
            // Emit one debounced signal (non-blocking so we never stall).
            let _ = tx.try_send(());
        }
    });

    Some((rx, watcher))
}

/// Display mode for entry references (inlay hints vs overlay vs off).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryRefDisplayMode {
    InlayHint,
    Overlay,
    Off,
}

/// Cached project-level analysis (populated on save).
struct ProjectState {
    phase1: Phase1Result,
    /// Phase2 result (available when phase1 has no errors).
    phase2: Option<Phase2Result>,
    /// Per-file token cache (lexed once during project analysis).
    token_cache: std::collections::HashMap<FileId, Vec<Token>>,
    /// Map from URI string to FileId in the phase1 source map.
    url_to_file_id: std::collections::HashMap<String, FileId>,
    /// Lint diagnostics from the linter.
    lint_diagnostics: Vec<LintDiagnostic>,
}

/// Run the LSP server on stdin/stdout.
pub fn run_server() {
    let sigint = sigint_channel();

    let (connection, io_threads) = Connection::stdio();

    let server_caps = serde_json::to_value(server_capabilities()).unwrap();
    let init_params = connection.initialize(server_caps).unwrap();
    let init_params: InitializeParams = serde_json::from_value(init_params).unwrap();

    let parent_exit = client_exit_channel(init_params.process_id);

    // Start file watcher if a workspace root is available.
    let watcher_pair = workspace_root_from_init(&init_params)
        .and_then(|root| file_watcher_channel(&root));
    let file_changed: Receiver<()> = match watcher_pair {
        Some((ref rx, _)) => rx.clone(),
        None => never(),
    };
    // Keep the watcher alive for the lifetime of the server.
    let _watcher = watcher_pair.map(|(_, w)| w);

    main_loop(&connection, init_params, &sigint, &parent_exit, &file_changed);

    // Drop the connection to close the channels, allowing IO threads to
    // finish.
    drop(connection);

    // io_threads.join() can hang if the stdin reader blocks.  Give it a
    // brief window before force-exiting.
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
        experimental: Some(serde_json::json!({
            "surfaceFormsProvider": true,
            "entryRefDisplayMode": true
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
    entry_ref_display_mode: EntryRefDisplayMode,
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
                let cache_root = path.parent().unwrap_or(&path);
                if let Some(proj) = build_project_state(&path, cache_root) {
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
                    let cache_root = path.parent().unwrap_or(&path);
                    if let Some(new_proj) = build_project_state(&path, cache_root) {
                        self.file_projects.insert(key, new_proj);
                    }
                }
            }
        }
    }
}

fn main_loop(
    connection: &Connection,
    init_params: InitializeParams,
    sigint: &Receiver<()>,
    parent_exit: &Receiver<()>,
    file_changed: &Receiver<()>,
) {
    let mut state = ServerState {
        documents: DocumentStore::default(),
        project: None,
        file_projects: std::collections::HashMap::new(),
        hut_projects: std::collections::HashMap::new(),
        init_params,
        entry_ref_display_mode: EntryRefDisplayMode::InlayHint,
    };

    // Try to discover and analyze the project on startup.
    if let Some(root) = workspace_root(&state.init_params) {
        state.project = try_analyze_project(&root);
    }

    loop {
        crossbeam_channel::select! {
            recv(connection.receiver) -> msg => {
                let msg = match msg {
                    Ok(msg) => msg,
                    Err(_) => {
                        return;
                    }
                };
                match msg {
                    Message::Request(req) => {
                        if req.method == "shutdown" {
                            let resp = Response::new_ok(req.id, ());
                            let _ = connection.sender.send(Message::Response(resp));
                            return;
                        }
                        if req.method == "hubullu/setEntryRefDisplayMode" {
                            let (resp, notifications) =
                                handle_set_display_mode(req.id.clone(), req, &mut state);
                            connection.sender.send(Message::Response(resp)).unwrap();
                            for n in notifications {
                                connection.sender.send(Message::Notification(n)).unwrap();
                            }
                        } else {
                            let resp = handle_request(req, &state);
                            connection.sender.send(Message::Response(resp)).unwrap();
                        }
                    }
                    Message::Notification(notif) => {
                        if notif.method == "exit" {
                            return;
                        }
                        let notifications = handle_notification(notif, &mut state);
                        for n in notifications {
                            connection.sender.send(Message::Notification(n)).unwrap();
                        }
                    }
                    Message::Response(_) => {}
                }
            }
            recv(sigint) -> _ => {
                return;
            }
            recv(parent_exit) -> _ => {
                return;
            }
            recv(file_changed) -> _ => {
                // A .hu / .hut file changed on disk — re-analyze and republish.
                if let Some(root) = workspace_root(&state.init_params) {
                    state.project = try_analyze_project(&root);
                }
                // Refresh hut projects for all open .hut documents.
                for uri_str in state.documents.uri_strings() {
                    if let Ok(uri) = uri_str.parse::<Uri>() {
                        if is_hut_uri(&uri) {
                            if let Some(doc) = state.documents.get(&uri) {
                                let text = doc.text.clone();
                                try_load_hut_project(&uri, &text, &mut state.hut_projects);
                            }
                        } else {
                            state.refresh_file_project(&uri);
                        }
                    }
                }
                // Republish diagnostics for every open document.
                for uri_str in state.documents.uri_strings() {
                    if let Ok(uri) = uri_str.parse::<Uri>() {
                        let project = state.project_for(&uri);
                        let n = publish_doc_diagnostics(&uri, &state.documents, project);
                        connection.sender.send(Message::Notification(n)).unwrap();
                    }
                }
            }
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
        "hubullu/surfaceForms" => handle_surface_forms(id, req, state),
        "hubullu/getEntryRefDisplayMode" => handle_get_display_mode(id, state),
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
        if is_hut_uri(uri) {
            semantic_tokens::generate_hut(
                &doc.parse_result.tokens,
                doc.parse_result.file_id, &doc.parse_result.source_map,
            )
        } else {
            semantic_tokens::generate(
                &doc.parse_result.tokens, &[],
                doc.parse_result.file_id, &doc.parse_result.source_map,
                &doc.parse_result.file,
            )
        }
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
        if is_hut_uri(uri) {
            let filename = convert::uri_to_filename(uri);
            document_link::hut_reference_links(
                &doc.text, &filename, project.map(|p| &p.phase1),
            )
        } else {
            document_link::document_links(
                &doc.parse_result, project.map(|p| &p.phase1),
            )
        }
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
    if s.entry_ref_display_mode != EntryRefDisplayMode::InlayHint {
        return Response::new_ok(id, serde_json::to_value(Option::<Vec<lsp_types::InlayHint>>::None).unwrap());
    }
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

fn handle_surface_forms(id: RequestId, req: Request, s: &ServerState) -> Response {
    if s.entry_ref_display_mode != EntryRefDisplayMode::Overlay {
        return Response::new_ok(
            id,
            serde_json::to_value(surface_forms::SurfaceFormsResult { items: vec![] }).unwrap(),
        );
    }
    let params: serde_json::Value = serde_json::from_value(req.params).unwrap();
    let uri_str = params["textDocument"]["uri"].as_str().unwrap_or("");
    let uri: Uri = match uri_str.parse() {
        Ok(u) => u,
        Err(_) => {
            return Response::new_ok(
                id,
                serde_json::to_value(surface_forms::SurfaceFormsResult { items: vec![] }).unwrap(),
            );
        }
    };
    let project = s.project_for(&uri);
    let items = (|| {
        let proj = project?;
        let p2 = proj.phase2.as_ref()?;
        let file_id = find_file_id(&uri, proj)?;
        if is_hut_uri(&uri) {
            let tokens = proj.token_cache.get(&file_id)?;
            Some(surface_forms::surface_forms_from_tokens(
                file_id, tokens, p2, &proj.phase1.source_map,
            ))
        } else {
            let file_ast = proj.phase1.files.get(&file_id)?;
            Some(surface_forms::surface_forms(file_id, file_ast, p2, &proj.phase1.source_map))
        }
    })()
    .unwrap_or_default();
    Response::new_ok(
        id,
        serde_json::to_value(surface_forms::SurfaceFormsResult { items }).unwrap(),
    )
}

fn handle_set_display_mode(
    id: RequestId,
    req: Request,
    state: &mut ServerState,
) -> (Response, Vec<Notification>) {
    let params: serde_json::Value = serde_json::from_value(req.params).unwrap();
    let mode_str = params["mode"].as_str().unwrap_or("inlayHint");
    state.entry_ref_display_mode = match mode_str {
        "overlay" => EntryRefDisplayMode::Overlay,
        "off" => EntryRefDisplayMode::Off,
        _ => EntryRefDisplayMode::InlayHint,
    };

    let mut notifications = Vec::new();
    // Send workspace/inlayHint/refresh notification.
    notifications.push(Notification::new(
        "workspace/inlayHint/refresh".into(),
        serde_json::Value::Null,
    ));
    // Send hubullu/surfaceFormsRefresh notification.
    notifications.push(Notification::new(
        "hubullu/surfaceFormsRefresh".into(),
        serde_json::Value::Null,
    ));

    (Response::new_ok(id, serde_json::Value::Null), notifications)
}

fn handle_get_display_mode(id: RequestId, state: &ServerState) -> Response {
    let mode = match state.entry_ref_display_mode {
        EntryRefDisplayMode::InlayHint => "inlayHint",
        EntryRefDisplayMode::Overlay => "overlay",
        EntryRefDisplayMode::Off => "off",
    };
    Response::new_ok(id, serde_json::json!({ "mode": mode }))
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

    // Start with single-file parse diagnostics (use doc's source_map).
    let parse_diags: Vec<&crate::error::Diagnostic> =
        doc.parse_result.diagnostics.iter().collect();

    // Project-level diagnostics (phase1 + lint) use the project source_map.
    let mut proj_diags: Vec<&crate::error::Diagnostic> = Vec::new();
    let proj_source_map;

    if let Some(proj) = project {
        if let Some(&fid) = proj.url_to_file_id.get(uri.as_str()) {
            let p1_diags: Vec<_> = proj
                .phase1
                .diagnostics
                .errors
                .iter()
                .filter(|d| d.labels.first().is_some_and(|l| l.span.file_id == fid))
                .collect();
            proj_diags.extend(p1_diags);

            // Include lint diagnostics for this file.
            let lint_diags: Vec<_> = proj
                .lint_diagnostics
                .iter()
                .filter(|ld| ld.diagnostic.labels.first().is_some_and(|l| l.span.file_id == fid))
                .map(|ld| &ld.diagnostic)
                .collect();
            proj_diags.extend(lint_diags);
        }
        proj_source_map = Some(&proj.phase1.source_map);
    } else {
        proj_source_map = None;
    }

    diagnostics::publish_combined_notification(
        uri,
        &parse_diags,
        &doc.parse_result.source_map,
        &proj_diags,
        proj_source_map,
    )
}

fn workspace_root(init_params: &InitializeParams) -> Option<PathBuf> {
    workspace_root_from_init(init_params)
}

fn workspace_root_from_init(init_params: &InitializeParams) -> Option<PathBuf> {
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
    build_project_state(&entry_path, root)
}

/// Build a complete ProjectState from an entry file path.
/// Tries disk cache first; if stale or missing, runs full analysis and saves.
///
/// `cache_root` is the directory under which `.hubullu-cache/` is placed
/// (workspace root for projects, entry file's parent for standalone files).
fn build_project_state(entry_path: &std::path::Path, cache_root: &std::path::Path) -> Option<ProjectState> {
    // Try disk cache first.
    if let Some(cached) = disk_cache::load(entry_path, cache_root) {
        let url_to_file_id = build_url_map(&cached.phase1.source_map);
        let lint_diagnostics = crate::lint::run_lint_from_phase1(&cached.phase1);
        return Some(ProjectState {
            phase1: cached.phase1,
            phase2: cached.phase2,
            token_cache: cached.token_cache,
            url_to_file_id,
            lint_diagnostics,
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
    disk_cache::save(entry_path, cache_root, &phase1, phase2.as_ref(), &token_cache);

    let lint_diagnostics = crate::lint::run_lint_from_phase1(&phase1);
    let url_to_file_id = build_url_map(&phase1.source_map);
    Some(ProjectState { phase1, phase2, token_cache, url_to_file_id, lint_diagnostics })
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

/// Load a project for a `.hut` file based on its `@reference` directives.
///
/// Parses the `.hut` file to extract `@reference` imports, then uses
/// [`phase1::run_phase1_virtual`] to build a unified project covering all
/// referenced `.hu` files with proper namespace imports.  The `.hut` file
/// itself is then injected into the project's source map, token cache, symbol
/// table, and file map so that LSP features work seamlessly.
fn try_load_hut_project(
    hut_uri: &Uri,
    hut_text: &str,
    hut_projects: &mut std::collections::HashMap<String, ProjectState>,
) {
    let hut_file_path = match convert::uri_to_path(hut_uri) {
        Some(p) => p,
        None => return,
    };
    let hut_dir = hut_file_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();

    // Parse the .hut to extract @reference directives.
    let hut_file = match crate::render::parse_hut(hut_text, &hut_file_path.to_string_lossy()) {
        Ok(h) => h,
        Err(_) => return,
    };
    if hut_file.references.is_empty() {
        return;
    }

    // Build a unified project from all @reference directives.
    let phase1 = crate::phase1::run_phase1_virtual(&hut_file.references, &hut_dir);

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

    let url_to_file_id = build_url_map(&phase1.source_map);

    let lint_diagnostics = crate::lint::run_lint_from_phase1(&phase1);
    let mut proj = ProjectState { phase1, phase2, token_cache, url_to_file_id, lint_diagnostics };

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

    // The virtual entry file's scope already has the correct imports from
    // run_phase1_virtual.  Copy that scope to the .hut file so symbol
    // resolution works for hover, go-to-definition, completion, etc.
    let virtual_path = hut_dir.join("<hut-virtual>");
    if let Some(&virtual_fid) = proj.phase1.path_to_id.get(&virtual_path) {
        if let Some(virtual_scope) = proj.phase1.symbol_table.scope(virtual_fid).cloned() {
            proj.phase1.symbol_table.scopes.insert(hut_file_id, virtual_scope);
        }
    }

    // Register an empty AST for the .hut file_id.
    proj.phase1.files.insert(
        hut_file_id,
        crate::ast::File { items: Vec::new() },
    );

    hut_projects.insert(hut_uri.as_str().to_string(), proj);
}
