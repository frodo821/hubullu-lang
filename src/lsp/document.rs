//! In-memory document store for open files.

use std::collections::HashMap;

use lsp_types::Uri;

use crate::ParseResult;

use super::convert;

/// State for a single open document.
pub struct DocumentState {
    pub text: String,
    pub version: i32,
    /// Cached parse result (refreshed on every change).
    pub parse_result: ParseResult,
}

/// Manages all open documents.
#[derive(Default)]
pub struct DocumentStore {
    documents: HashMap<String, DocumentState>,
}

impl DocumentStore {
    pub fn open(&mut self, uri: &Uri, text: String, version: i32) {
        let filename = convert::uri_to_filename(uri);
        let parse_result = crate::parse_source(&text, &filename);
        self.documents.insert(uri.as_str().to_string(), DocumentState {
            text,
            version,
            parse_result,
        });
    }

    pub fn change(&mut self, uri: &Uri, text: String, version: i32) {
        let filename = convert::uri_to_filename(uri);
        let parse_result = crate::parse_source(&text, &filename);
        let key = uri.as_str().to_string();
        if let Some(doc) = self.documents.get_mut(&key) {
            doc.text = text;
            doc.version = version;
            doc.parse_result = parse_result;
        } else {
            self.documents.insert(key, DocumentState {
                text,
                version,
                parse_result,
            });
        }
    }

    pub fn close(&mut self, uri: &Uri) {
        self.documents.remove(uri.as_str());
    }

    pub fn get(&self, uri: &Uri) -> Option<&DocumentState> {
        self.documents.get(uri.as_str())
    }
}
