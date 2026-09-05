mod completion;
mod documents;
mod line_index;
mod navigation;
mod semantic;
mod symbols;

use std::sync::Mutex;

pub use documents::{DocumentSnapshot, DocumentStore};
pub use line_index::LineIndex;
use radiant_compiler::Diagnostic as CompilerDiagnostic;
use tokio::io::{AsyncRead, AsyncWrite};
use tower_lsp::{
    Client, LanguageServer, LspService, Server,
    jsonrpc::Result,
    lsp_types::{
        CompletionOptions, CompletionParams, CompletionResponse, Diagnostic, DiagnosticSeverity,
        DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
        DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
        Hover, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
        InitializedParams, MessageType, NumberOrString, OneOf, ServerCapabilities, ServerInfo,
        TextDocumentSyncCapability, TextDocumentSyncKind, Url,
    },
};

/// Radiant language-server state and protocol handlers.
pub struct Backend {
    client: Client,
    documents: Mutex<DocumentStore>,
}

impl Backend {
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: Mutex::new(DocumentStore::default()),
        }
    }

    async fn publish(&self, uri: Url, version: Option<i32>, diagnostics: Vec<Diagnostic>) {
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
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
        Ok(documents
            .get(&params.text_document_position.text_document.uri)
            .map(|snapshot| {
                CompletionResponse::Array(completion::completions(
                    snapshot,
                    params.text_document_position.position,
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
        let document = &params.text_document_position_params;
        Ok(documents
            .get(&document.text_document.uri)
            .and_then(|snapshot| {
                navigation::definition(snapshot, &document.text_document.uri, document.position)
            }))
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let document = params.text_document;
        let diagnostics = {
            let mut documents = self.documents.lock().expect("document store poisoned");
            let snapshot = documents.open(document.uri.clone(), document.version, document.text);
            lsp_diagnostics(snapshot)
        };
        self.publish(document.uri, Some(document.version), diagnostics)
            .await;
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
        let diagnostics = {
            let mut documents = self.documents.lock().expect("document store poisoned");
            documents
                .change(&document.uri, document.version, text)
                .map(lsp_diagnostics)
        };
        if let Some(diagnostics) = diagnostics {
            self.publish(document.uri, Some(document.version), diagnostics)
                .await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents
            .lock()
            .expect("document store poisoned")
            .close(&uri);
        self.publish(uri, None, Vec::new()).await;
    }
}

fn lsp_diagnostics(snapshot: &DocumentSnapshot) -> Vec<Diagnostic> {
    snapshot
        .analysis
        .diagnostics
        .iter()
        .map(|diagnostic| map_diagnostic(diagnostic, &snapshot.line_index))
        .collect()
}

fn map_diagnostic(diagnostic: &CompilerDiagnostic, lines: &LineIndex) -> Diagnostic {
    Diagnostic {
        range: lines.span_to_range(diagnostic.span),
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String(diagnostic.code.into())),
        source: Some("radiant".into()),
        message: diagnostic.message.clone(),
        ..Diagnostic::default()
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
    use radiant_compiler::analyze;
    use tower_lsp::lsp_types::{DiagnosticSeverity, NumberOrString, Position};

    use super::{LineIndex, map_diagnostic};

    #[test]
    fn maps_all_compiler_diagnostic_fields_and_utf16_range() {
        let source = "😀 {broken +}";
        let compiler = analyze("unicode", source).diagnostics.remove(0);
        let diagnostic = map_diagnostic(&compiler, &LineIndex::new(source));

        assert_eq!(
            diagnostic.code,
            Some(NumberOrString::String(compiler.code.into()))
        );
        assert_eq!(diagnostic.message, compiler.message);
        assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diagnostic.source.as_deref(), Some("radiant"));
        assert_eq!(diagnostic.range.start, Position::new(0, 12));
        assert_eq!(diagnostic.range.end, Position::new(0, 12));
    }
}
