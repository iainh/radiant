mod completion;
mod diagnostics;
mod documents;
mod line_index;
mod navigation;
mod semantic;
mod symbols;
mod workspace;

use std::{collections::BTreeMap, sync::Mutex};

pub use documents::{DocumentSnapshot, DocumentStore};
pub use line_index::LineIndex;
use tokio::io::{AsyncRead, AsyncWrite};
use tower_lsp::{
    Client, LanguageServer, LspService, Server,
    jsonrpc::Result,
    lsp_types::{
        CompletionOptions, CompletionParams, CompletionResponse, Diagnostic,
        DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
        DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
        DidOpenTextDocumentParams, DocumentSymbolParams, DocumentSymbolResponse, FileSystemWatcher,
        GlobPattern, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams,
        HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams,
        MessageType, OneOf, Registration, ServerCapabilities, ServerInfo,
        TextDocumentSyncCapability, TextDocumentSyncKind, Url,
    },
};

use workspace::WorkspaceIndex;

/// Radiant language-server state and protocol handlers.
pub struct Backend {
    client: Client,
    documents: Mutex<DocumentStore>,
    workspace: Mutex<WorkspaceIndex>,
    register_file_watcher: Mutex<bool>,
    snippet_completion: Mutex<bool>,
}

impl Backend {
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: Mutex::new(DocumentStore::default()),
            workspace: Mutex::new(WorkspaceIndex::default()),
            register_file_watcher: Mutex::new(false),
            snippet_completion: Mutex::new(false),
        }
    }

    async fn publish(&self, uri: Url, version: Option<i32>, diagnostics: Vec<Diagnostic>) {
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }

    fn open_diagnostics(&self) -> Vec<(Url, i32, Vec<Diagnostic>)> {
        let documents = self.documents.lock().expect("document store poisoned");
        let workspace = self.workspace.lock().expect("workspace index poisoned");
        documents
            .iter()
            .map(|(uri, snapshot)| {
                (
                    uri.clone(),
                    snapshot.version,
                    diagnostics::diagnostics(uri, snapshot, &documents, &workspace),
                )
            })
            .collect()
    }

    async fn publish_open_diagnostics(&self) {
        for (uri, version, diagnostics) in self.open_diagnostics() {
            self.publish(uri, Some(version), diagnostics).await;
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let dynamic_watching = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.did_change_watched_files.as_ref())
            .and_then(|watching| watching.dynamic_registration)
            .unwrap_or(false);
        *self
            .register_file_watcher
            .lock()
            .expect("file watcher state poisoned") = dynamic_watching;
        *self
            .snippet_completion
            .lock()
            .expect("snippet completion state poisoned") = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text| text.completion.as_ref())
            .and_then(|completion| completion.completion_item.as_ref())
            .and_then(|item| item.snippet_support)
            .unwrap_or(false);
        let roots: Vec<_> = params
            .workspace_folders
            .filter(|folders| !folders.is_empty())
            .map(|folders| folders.into_iter().map(|folder| folder.uri).collect())
            .unwrap_or_else(|| params.root_uri.into_iter().collect());
        self.workspace
            .lock()
            .expect("workspace index poisoned")
            .set_roots(roots);

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                document_symbol_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["{".into(), "#".into()]),
                    ..CompletionOptions::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "radiant-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        let register = *self
            .register_file_watcher
            .lock()
            .expect("file watcher state poisoned");
        if register {
            let options = DidChangeWatchedFilesRegistrationOptions {
                watchers: vec![FileSystemWatcher {
                    glob_pattern: GlobPattern::String("**/templates/**".into()),
                    kind: None,
                }],
            };
            let registration = Registration {
                id: "radiant-template-files".into(),
                method: "workspace/didChangeWatchedFiles".into(),
                register_options: Some(
                    serde_json::to_value(options).expect("watch options serialize"),
                ),
            };
            if let Err(error) = self.client.register_capability(vec![registration]).await {
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("failed to register template file watcher: {error}"),
                    )
                    .await;
            }
        }
        self.client
            .log_message(MessageType::INFO, "Radiant language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let documents = self.documents.lock().expect("document store poisoned");
        Ok(documents
            .get(&params.text_document.uri)
            .map(|snapshot| DocumentSymbolResponse::Nested(symbols::document_symbols(snapshot))))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let documents = self.documents.lock().expect("document store poisoned");
        let workspace = self.workspace.lock().expect("workspace index poisoned");
        let uri = &params.text_document_position.text_document.uri;
        let mut analyses = workspace
            .analyses(uri)
            .into_iter()
            .map(|(id, analysis)| (id.to_owned(), analysis))
            .collect::<BTreeMap<_, _>>();
        for (open_uri, open) in documents.iter() {
            if workspace.shares_root(uri, open_uri)
                && let Some(id) = workspace.template_id(open_uri)
            {
                analyses.insert(id, &open.analysis);
            }
        }
        let snippets = *self
            .snippet_completion
            .lock()
            .expect("snippet completion state poisoned");
        Ok(documents.get(uri).map(|snapshot| {
            CompletionResponse::Array(completion::completions(
                snapshot,
                params.text_document_position.position,
                &analyses,
                snippets,
            ))
        }))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let documents = self.documents.lock().expect("document store poisoned");
        Ok(documents
            .get(&params.text_document_position_params.text_document.uri)
            .and_then(|snapshot| {
                navigation::hover(snapshot, params.text_document_position_params.position)
            }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let documents = self.documents.lock().expect("document store poisoned");
        let workspace = self.workspace.lock().expect("workspace index poisoned");
        let document = &params.text_document_position_params;
        Ok(documents
            .get(&document.text_document.uri)
            .and_then(|snapshot| {
                navigation::definition(
                    snapshot,
                    &document.text_document.uri,
                    document.position,
                    &workspace,
                )
            }))
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        self.workspace
            .lock()
            .expect("workspace index poisoned")
            .refresh_affected(params.changes.into_iter().map(|change| change.uri));
        self.publish_open_diagnostics().await;
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let document = params.text_document;
        self.documents
            .lock()
            .expect("document store poisoned")
            .open(document.uri, document.version, document.text);
        self.publish_open_diagnostics().await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let document = params.text_document;
        let Some(text) = params
            .content_changes
            .last()
            .map(|change| change.text.clone())
        else {
            return;
        };
        let changed = {
            let mut documents = self.documents.lock().expect("document store poisoned");
            documents
                .change(&document.uri, document.version, text)
                .is_some()
        };
        if changed {
            self.publish_open_diagnostics().await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents
            .lock()
            .expect("document store poisoned")
            .close(&uri);
        self.publish(uri, None, Vec::new()).await;
        self.publish_open_diagnostics().await;
    }
}

/// Serves the Radiant language server over an arbitrary asynchronous transport.
pub async fn serve<I, O>(input: I, output: O)
where
    I: AsyncRead + Unpin,
    O: AsyncWrite,
{
    let (service, socket) = LspService::new(Backend::new);
    Server::new(input, output, socket).serve(service).await;
}
