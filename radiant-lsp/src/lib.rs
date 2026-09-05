mod completion;
mod diagnostics;
mod documents;
mod line_index;
mod navigation;
mod semantic;
mod symbols;
mod workspace;

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

pub use documents::{DocumentSnapshot, DocumentStore};
pub use line_index::LineIndex;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
    time::{Instant, sleep_until},
};
use tower_lsp::{
    Client, LanguageServer, LspService, Server,
    jsonrpc::Result,
    lsp_types::{
        CompletionOptions, CompletionParams, CompletionResponse, Diagnostic,
        DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
        DidChangeWatchedFilesRegistrationOptions, DidChangeWorkspaceFoldersParams,
        DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentSymbolParams,
        DocumentSymbolResponse, FileEvent, FileSystemWatcher, GlobPattern, GotoDefinitionParams,
        GotoDefinitionResponse, Hover, HoverParams, HoverProviderCapability, InitializeParams,
        InitializeResult, InitializedParams, MessageType, OneOf, ReferenceParams, Registration,
        ServerCapabilities, ServerInfo, SymbolInformation, TextDocumentSyncCapability,
        TextDocumentSyncKind, Url, WorkspaceFoldersServerCapabilities, WorkspaceServerCapabilities,
        WorkspaceSymbolParams,
    },
};

use workspace::WorkspaceIndex;

/// Radiant language-server state and protocol handlers.
pub struct Backend {
    client: Client,
    state: Arc<State>,
    watched_changes: mpsc::UnboundedSender<Vec<FileEvent>>,
    register_file_watcher: Mutex<bool>,
    snippet_completion: Mutex<bool>,
}

struct State {
    documents: Mutex<DocumentStore>,
    workspace: Mutex<WorkspaceIndex>,
}

const WATCH_DEBOUNCE: Duration = Duration::from_millis(50);

impl Backend {
    #[must_use]
    pub fn new(client: Client) -> Self {
        let state = Arc::new(State {
            documents: Mutex::new(DocumentStore::default()),
            workspace: Mutex::new(WorkspaceIndex::default()),
        });
        let (watched_changes, receiver) = mpsc::unbounded_channel();
        tokio::spawn(process_watched_changes(
            client.clone(),
            Arc::clone(&state),
            receiver,
        ));
        Self {
            client,
            state,
            watched_changes,
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
        open_diagnostics(&self.state)
    }

    async fn publish_open_diagnostics(&self) {
        for (uri, version, diagnostics) in self.open_diagnostics() {
            self.publish(uri, Some(version), diagnostics).await;
        }
    }
}

fn open_diagnostics(state: &State) -> Vec<(Url, i32, Vec<Diagnostic>)> {
    let documents = state.documents.lock().expect("document store poisoned");
    let workspace = state.workspace.lock().expect("workspace index poisoned");
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

async fn next_watched_batch(
    receiver: &mut mpsc::UnboundedReceiver<Vec<FileEvent>>,
    debounce: Duration,
) -> Option<Vec<FileEvent>> {
    let first = receiver.recv().await?;
    let mut changes = first
        .into_iter()
        .map(|event| (event.uri.clone(), event))
        .collect::<BTreeMap<_, _>>();
    let deadline = sleep_until(Instant::now() + debounce);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            batch = receiver.recv() => {
                let Some(batch) = batch else {
                    break;
                };
                for event in batch {
                    changes.insert(event.uri.clone(), event);
                }
                deadline.as_mut().reset(Instant::now() + debounce);
            }
            () = &mut deadline => break,
        }
    }
    Some(changes.into_values().collect())
}

async fn process_watched_changes(
    client: Client,
    state: Arc<State>,
    mut receiver: mpsc::UnboundedReceiver<Vec<FileEvent>>,
) {
    while let Some(changes) = next_watched_batch(&mut receiver, WATCH_DEBOUNCE).await {
        state
            .workspace
            .lock()
            .expect("workspace index poisoned")
            .update_affected(changes);
        for (uri, version, diagnostics) in open_diagnostics(&state) {
            client
                .publish_diagnostics(uri, diagnostics, Some(version))
                .await;
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
        self.state
            .workspace
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
                references_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    ..WorkspaceServerCapabilities::default()
                }),
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
        let documents = self
            .state
            .documents
            .lock()
            .expect("document store poisoned");
        Ok(documents
            .get(&params.text_document.uri)
            .map(|snapshot| DocumentSymbolResponse::Nested(symbols::document_symbols(snapshot))))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let documents = self
            .state
            .documents
            .lock()
            .expect("document store poisoned");
        let workspace = self
            .state
            .workspace
            .lock()
            .expect("workspace index poisoned");
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
        let documents = self
            .state
            .documents
            .lock()
            .expect("document store poisoned");
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
        let documents = self
            .state
            .documents
            .lock()
            .expect("document store poisoned");
        let workspace = self
            .state
            .workspace
            .lock()
            .expect("workspace index poisoned");
        let document = &params.text_document_position_params;
        Ok(documents
            .get(&document.text_document.uri)
            .and_then(|snapshot| {
                navigation::definition(
                    snapshot,
                    &document.text_document.uri,
                    document.position,
                    &workspace,
                    &documents,
                )
            }))
    }

    async fn references(
        &self,
        params: ReferenceParams,
    ) -> Result<Option<Vec<tower_lsp::lsp_types::Location>>> {
        let documents = self
            .state
            .documents
            .lock()
            .expect("document store poisoned");
        let workspace = self
            .state
            .workspace
            .lock()
            .expect("workspace index poisoned");
        let document = &params.text_document_position;
        Ok(documents
            .get(&document.text_document.uri)
            .and_then(|snapshot| {
                navigation::references(
                    snapshot,
                    &document.text_document.uri,
                    document.position,
                    params.context.include_declaration,
                    &workspace,
                    &documents,
                )
            }))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let documents = self
            .state
            .documents
            .lock()
            .expect("document store poisoned");
        let workspace = self
            .state
            .workspace
            .lock()
            .expect("workspace index poisoned");
        Ok(Some(symbols::workspace_symbols(
            &params.query,
            &workspace,
            &documents,
        )))
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let _ = self.watched_changes.send(params.changes);
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        self.state
            .workspace
            .lock()
            .expect("workspace index poisoned")
            .change_roots(
                params.event.added.into_iter().map(|folder| folder.uri),
                params.event.removed.into_iter().map(|folder| folder.uri),
            );
        self.publish_open_diagnostics().await;
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let document = params.text_document;
        self.state
            .documents
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
            let mut documents = self
                .state
                .documents
                .lock()
                .expect("document store poisoned");
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
        self.state
            .documents
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::mpsc;
    use tower_lsp::lsp_types::{FileChangeType, FileEvent, Url};

    use super::next_watched_batch;

    #[tokio::test(start_paused = true)]
    async fn watched_file_bursts_coalesce_by_uri_without_sleeping() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let first = Url::parse("file:///workspace/templates/first.html").unwrap();
        let second = Url::parse("file:///workspace/templates/second.html").unwrap();
        sender
            .send(vec![FileEvent::new(first.clone(), FileChangeType::CREATED)])
            .unwrap();
        sender
            .send(vec![
                FileEvent::new(first.clone(), FileChangeType::CHANGED),
                FileEvent::new(second.clone(), FileChangeType::CREATED),
            ])
            .unwrap();

        let batch = next_watched_batch(&mut receiver, Duration::from_millis(50))
            .await
            .unwrap();

        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0], FileEvent::new(first, FileChangeType::CHANGED));
        assert_eq!(batch[1], FileEvent::new(second, FileChangeType::CREATED));
    }
}
