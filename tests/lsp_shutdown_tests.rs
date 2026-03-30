//! Tests for LSP shutdown behaviour.
//!
//! These tests spawn `hubullu lsp` as a subprocess and communicate with it
//! over stdin/stdout using the LSP JSON-RPC protocol.  The goal is to verify
//! that the server shuts down promptly after receiving a `shutdown` request,
//! regardless of whether the client sends the subsequent `exit` notification.

#![cfg(feature = "lsp")]

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Encode a JSON-RPC message with the Content-Length header.
fn encode(body: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

/// Read one LSP message from the reader, returning the JSON body.
/// Blocks until a full message is available.
fn read_message(reader: &mut BufReader<impl Read>) -> String {
    // Read headers until we find Content-Length.
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("failed to read header line");
        let line = line.trim();
        if line.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = line.strip_prefix("Content-Length: ") {
            content_length = Some(rest.parse().expect("bad Content-Length"));
        }
    }

    let len = content_length.expect("no Content-Length header");
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).expect("failed to read body");
    String::from_utf8(buf).expect("body is not UTF-8")
}

/// Find the `hubullu` binary built by cargo.
fn hubullu_bin() -> std::path::PathBuf {
    // `cargo test` sets this env var to the directory containing built test
    // artefacts; the binary lives alongside them.
    let mut path = std::env::current_exe()
        .expect("current_exe")
        .parent()
        .expect("parent")
        .parent()
        .expect("parent")
        .to_path_buf();
    path.push("hubullu");
    if !path.exists() {
        // Fallback: try cargo build target/debug
        path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug")
            .join("hubullu");
    }
    assert!(path.exists(), "hubullu binary not found at {:?}", path);
    path
}

/// Spawn the LSP server subprocess and perform the initialize handshake.
/// Returns the child process and a buffered reader over its stdout.
fn spawn_and_initialize() -> (std::process::Child, BufReader<std::process::ChildStdout>) {
    let mut child = Command::new(hubullu_bin())
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn hubullu lsp");

    let stdin = child.stdin.as_mut().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // --- initialize request (id=1) ---
    let init_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": null,
            "capabilities": {},
            "rootUri": null
        }
    });
    stdin
        .write_all(&encode(&init_request.to_string()))
        .expect("write initialize");
    stdin.flush().unwrap();

    // Read the initialize response.
    let resp = read_message(&mut reader);
    let resp: serde_json::Value = serde_json::from_str(&resp).expect("parse init response");
    assert_eq!(resp["id"], 1, "expected initialize response with id=1");

    // --- initialized notification ---
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    stdin
        .write_all(&encode(&initialized.to_string()))
        .expect("write initialized");
    stdin.flush().unwrap();

    (child, reader)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// When the client sends `shutdown` followed by `exit`, the server should
/// terminate promptly (well within 5 seconds).
#[test]
fn shutdown_then_exit_terminates_promptly() {
    let (mut child, _reader) = spawn_and_initialize();
    let stdin = child.stdin.as_mut().unwrap();

    // --- shutdown request (id=2) ---
    let shutdown = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "shutdown",
        "params": null
    });
    stdin
        .write_all(&encode(&shutdown.to_string()))
        .expect("write shutdown");
    stdin.flush().unwrap();

    // --- exit notification ---
    let exit = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "exit",
        "params": null
    });
    stdin
        .write_all(&encode(&exit.to_string()))
        .expect("write exit");
    stdin.flush().unwrap();

    // The server should terminate within 2 seconds.
    let start = Instant::now();
    let deadline = Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                // Process exited — success.
                return;
            }
            Ok(None) => {
                if start.elapsed() > deadline {
                    child.kill().ok();
                    panic!(
                        "server did not terminate within {:?} after shutdown+exit",
                        deadline
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("error waiting for process: {}", e),
        }
    }
}

/// When the client sends `shutdown` but does NOT send `exit`, the server
/// should still terminate within a reasonable time (< 5 seconds).
///
/// This test demonstrates the current bug: `handle_shutdown` blocks for up
/// to 30 seconds waiting for the `exit` notification, which exceeds the
/// typical 5-second graceful shutdown window imposed by editors.
#[test]
fn shutdown_without_exit_terminates_within_five_seconds() {
    let (mut child, mut reader) = spawn_and_initialize();
    let stdin = child.stdin.as_mut().unwrap();

    // --- shutdown request (id=2) ---
    let shutdown = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "shutdown",
        "params": null
    });
    stdin
        .write_all(&encode(&shutdown.to_string()))
        .expect("write shutdown");
    stdin.flush().unwrap();

    // Read the shutdown response to confirm it was received.
    let resp = read_message(&mut reader);
    let resp: serde_json::Value = serde_json::from_str(&resp).expect("parse shutdown response");
    assert_eq!(resp["id"], 2, "expected shutdown response with id=2");

    // Do NOT send exit.  Close stdin to signal EOF.
    drop(child.stdin.take());

    // The server should terminate within 5 seconds (the graceful period).
    let start = Instant::now();
    let deadline = Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                let elapsed = start.elapsed();
                eprintln!("server exited after {:.1}s (no exit notification)", elapsed.as_secs_f64());
                return;
            }
            Ok(None) => {
                if start.elapsed() > deadline {
                    child.kill().ok();
                    child.wait().ok();
                    panic!(
                        "BUG: server did not terminate within {:?} after shutdown \
                         (without exit notification). handle_shutdown() blocks for \
                         up to 30s waiting for an exit notification that never comes.",
                        deadline
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("error waiting for process: {}", e),
        }
    }
}

/// When the client sends `shutdown` but keeps stdin open (no `exit`, no EOF),
/// the server should still terminate within 5 seconds.
///
/// This simulates an editor that sends `shutdown`, receives the response, and
/// then waits for the process to exit — only sending SIGTERM after a grace
/// period.  If `handle_shutdown` blocks for 30s waiting for `exit`, the server
/// will exceed the grace period.
#[test]
fn shutdown_without_exit_and_stdin_open_terminates_within_five_seconds() {
    let (mut child, mut reader) = spawn_and_initialize();

    {
        let stdin = child.stdin.as_mut().unwrap();

        // --- shutdown request (id=2) ---
        let shutdown = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "shutdown",
            "params": null
        });
        stdin
            .write_all(&encode(&shutdown.to_string()))
            .expect("write shutdown");
        stdin.flush().unwrap();

        // Read the shutdown response.
        let resp = read_message(&mut reader);
        let resp: serde_json::Value =
            serde_json::from_str(&resp).expect("parse shutdown response");
        assert_eq!(resp["id"], 2);
    }

    // Intentionally do NOT close stdin and do NOT send exit.
    // Keep a reference to stdin alive so the pipe stays open.
    // (child.stdin is still Some)

    let start = Instant::now();
    let deadline = Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                let elapsed = start.elapsed();
                eprintln!(
                    "server exited after {:.1}s (no exit, stdin still open)",
                    elapsed.as_secs_f64()
                );
                return;
            }
            Ok(None) => {
                if start.elapsed() > deadline {
                    child.kill().ok();
                    child.wait().ok();
                    panic!(
                        "BUG: server did not terminate within {:?} after shutdown \
                         (stdin still open, no exit notification). \
                         handle_shutdown() blocks for 30s waiting for exit.",
                        deadline
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("error waiting for process: {}", e),
        }
    }
}

/// When stdin remains open and no `exit` is sent, the server eventually
/// terminates but takes ~30 seconds (the `handle_shutdown` timeout).
/// This test measures the actual time to confirm the 30s block.
#[test]
fn shutdown_without_exit_stdin_open_measures_actual_delay() {
    let (mut child, mut reader) = spawn_and_initialize();

    {
        let stdin = child.stdin.as_mut().unwrap();
        let shutdown = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "shutdown",
            "params": null
        });
        stdin
            .write_all(&encode(&shutdown.to_string()))
            .expect("write shutdown");
        stdin.flush().unwrap();

        let resp = read_message(&mut reader);
        let resp: serde_json::Value =
            serde_json::from_str(&resp).expect("parse shutdown response");
        assert_eq!(resp["id"], 2);
    }

    // Do NOT close stdin, do NOT send exit.  Wait up to 35s.
    let start = Instant::now();
    let hard_deadline = Duration::from_secs(35);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let elapsed = start.elapsed();
                eprintln!(
                    "server exited after {:.1}s, status: {:?} (stdin open, no exit)",
                    elapsed.as_secs_f64(),
                    status
                );
                // The server should NOT take more than 5s.
                // If it does, that's the bug.
                assert!(
                    elapsed < Duration::from_secs(5),
                    "BUG: server took {:.1}s to exit (expected < 5s). \
                     handle_shutdown blocks for 30s waiting for exit notification.",
                    elapsed.as_secs_f64()
                );
                return;
            }
            Ok(None) => {
                if start.elapsed() > hard_deadline {
                    child.kill().ok();
                    child.wait().ok();
                    panic!("server did not exit even after 35s");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => panic!("error waiting for process: {}", e),
        }
    }
}

/// After receiving `shutdown`, the server should still respond to the
/// shutdown request (i.e., send back a result for the shutdown request ID)
/// before terminating.
#[test]
fn shutdown_response_is_sent() {
    let (mut child, mut reader) = spawn_and_initialize();
    let stdin = child.stdin.as_mut().unwrap();

    // --- shutdown request (id=42) ---
    let shutdown = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "shutdown",
        "params": null
    });
    stdin
        .write_all(&encode(&shutdown.to_string()))
        .expect("write shutdown");
    stdin.flush().unwrap();

    // We should receive the shutdown response.
    let resp = read_message(&mut reader);
    let resp: serde_json::Value = serde_json::from_str(&resp).expect("parse shutdown response");
    assert_eq!(resp["id"], 42, "shutdown response should have the same id");
    assert!(
        resp.get("error").is_none(),
        "shutdown response should not be an error: {:?}",
        resp
    );

    // Clean up: send exit so the server doesn't hang.
    let exit = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "exit",
        "params": null
    });
    stdin
        .write_all(&encode(&exit.to_string()))
        .expect("write exit");
    stdin.flush().unwrap();

    // Wait for exit.
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > Duration::from_secs(3) => {
                child.kill().ok();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break,
        }
    }
}
