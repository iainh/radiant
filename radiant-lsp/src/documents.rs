use std::{collections::HashMap, path::PathBuf};

use radiant_compiler::{Analysis, analyze};
use tower_lsp::lsp_types::Url;

use crate::LineIndex;

/// An immutable analysis snapshot for an open document.
#[derive(Debug)]
pub struct DocumentSnapshot {
    pub text: String,
    pub version: i32,
    pub analysis: Analysis,
    pub line_index: LineIndex,
    pub template_root: Option<PathBuf>,
}

impl DocumentSnapshot {
    fn new(uri: &Url, version: i32, text: String) -> Self {
        let analysis = analyze(uri.as_str(), &text);
        let line_index = LineIndex::new(&text);
        Self {
            text,
            version,
            analysis,
            line_index,
            template_root: template_root(uri),
        }
    }
}

/// Owns the current contents and analyses of all open documents.
#[derive(Debug, Default)]
pub struct DocumentStore {
    documents: HashMap<Url, DocumentSnapshot>,
}

impl DocumentStore {
    pub fn open(&mut self, uri: Url, version: i32, text: String) -> &DocumentSnapshot {
        self.documents
            .insert(uri.clone(), DocumentSnapshot::new(&uri, version, text));
        &self.documents[&uri]
    }

    /// Applies a full-document update, rejecting versions that are not newer.
    pub fn change(&mut self, uri: &Url, version: i32, text: String) -> Option<&DocumentSnapshot> {
        let current = self.documents.get(uri)?;
        if version <= current.version {
            return None;
        }
        self.documents
            .insert(uri.clone(), DocumentSnapshot::new(uri, version, text));
        self.documents.get(uri)
    }

    pub fn close(&mut self, uri: &Url) -> Option<DocumentSnapshot> {
        self.documents.remove(uri)
    }

    #[must_use]
    pub fn get(&self, uri: &Url) -> Option<&DocumentSnapshot> {
        self.documents.get(uri)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Url, &DocumentSnapshot)> {
        self.documents.iter()
    }
}

fn template_root(uri: &Url) -> Option<PathBuf> {
    let path = uri.to_file_path().ok()?;
    path.ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "templates"))
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::Url;

    use super::DocumentStore;

    #[test]
    fn open_change_reject_stale_and_close_documents() {
        let uri = Url::parse("file:///workspace/templates/pages/index.html").unwrap();
        let mut documents = DocumentStore::default();

        let opened = documents.open(uri.clone(), 1, "{broken +}".into());
        assert_eq!(opened.version, 1);
        assert_eq!(opened.analysis.diagnostics.len(), 1);
        assert_eq!(
            opened.template_root.as_deref(),
            Some(std::path::Path::new("/workspace/templates"))
        );

        let changed = documents.change(&uri, 2, "valid {name}".into()).unwrap();
        assert_eq!(changed.version, 2);
        assert!(changed.analysis.diagnostics.is_empty());

        assert!(documents.change(&uri, 2, "{stale +}".into()).is_none());
        assert_eq!(documents.get(&uri).unwrap().text, "valid {name}");

        assert!(documents.close(&uri).is_some());
        assert!(documents.get(&uri).is_none());
    }

    #[test]
    fn ignores_changes_for_documents_that_are_not_open() {
        let uri = Url::parse("file:///workspace/outside.html").unwrap();
        let mut documents = DocumentStore::default();

        assert!(documents.change(&uri, 1, "text".into()).is_none());
        assert!(documents.get(&uri).is_none());
    }
}
