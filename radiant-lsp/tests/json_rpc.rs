use std::{fs, process::Stdio, time::Duration};

use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout, Command},
    time::timeout,
};

async fn send(stdin: &mut ChildStdin, message: Value) {
    let body = serde_json::to_vec(&message).unwrap();
    stdin
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await
        .unwrap();
    stdin.write_all(&body).await.unwrap();
    stdin.flush().await.unwrap();
}

async fn receive(stdout: &mut BufReader<ChildStdout>) -> Value {
    timeout(Duration::from_secs(5), async {
        let mut content_length = None;
        loop {
            let mut header = String::new();
            stdout.read_line(&mut header).await.unwrap();
            if header == "\r\n" {
                break;
            }
            if let Some(value) = header.strip_prefix("Content-Length:") {
                content_length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0; content_length.unwrap()];
        stdout.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    })
    .await
    .expect("timed out waiting for server message")
}

async fn receive_diagnostics(stdout: &mut BufReader<ChildStdout>, uri: &str) -> Value {
    loop {
        let message = receive(stdout).await;
        if message["method"] == "textDocument/publishDiagnostics" && message["params"]["uri"] == uri
        {
            return message;
        }
    }
}

async fn receive_response(stdout: &mut BufReader<ChildStdout>, id: i64) -> Value {
    loop {
        let message = receive(stdout).await;
        if message["id"] == id {
            return message;
        }
    }
}

#[tokio::test]
async fn stdio_server_publishes_updates_rejects_stale_versions_and_clears_on_close() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_radiant-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let uri = "file:///workspace/templates/page.html";

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )
    .await;
    let initialized = receive(&mut stdout).await;
    assert_eq!(initialized["id"], 1);
    assert_eq!(initialized["result"]["capabilities"]["textDocumentSync"], 1);
    assert_eq!(
        initialized["result"]["capabilities"]["documentSymbolProvider"],
        true
    );
    assert_eq!(
        initialized["result"]["capabilities"]["completionProvider"]["triggerCharacters"],
        json!(["{", "#"])
    );
    assert_eq!(initialized["result"]["capabilities"]["hoverProvider"], true);
    assert_eq!(
        initialized["result"]["capabilities"]["definitionProvider"],
        true
    );
    assert_eq!(
        initialized["result"]["capabilities"]["referencesProvider"],
        true
    );
    assert_eq!(
        initialized["result"]["capabilities"]["workspaceSymbolProvider"],
        true
    );

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"radiant","version":1,"text":"😀 {broken +}"}}}),
    )
    .await;
    let opened = receive(&mut stdout).await;
    assert_eq!(opened["method"], "textDocument/publishDiagnostics");
    assert_eq!(opened["params"]["version"], 1);
    assert_eq!(
        opened["params"]["diagnostics"][0]["code"],
        "E_EXPR_EXPECTED"
    );
    assert_eq!(opened["params"]["diagnostics"][0]["severity"], 1);
    assert_eq!(
        opened["params"]["diagnostics"][0]["range"]["start"]["character"],
        12
    );

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"😀{@String name}\n{#if name}{name}{#else}none{/if}"}]}}),
    )
    .await;
    let changed = receive(&mut stdout).await;
    assert_eq!(changed["params"]["version"], 2);
    assert_eq!(changed["params"]["diagnostics"], json!([]));

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/documentSymbol","params":{"textDocument":{"uri":uri}}}),
    )
    .await;
    let symbols = receive(&mut stdout).await;
    assert_eq!(symbols["id"], 3);
    assert!(symbols["result"].is_array());
    assert_eq!(symbols["result"][0]["name"], "name");
    assert_eq!(
        symbols["result"][0]["selectionRange"]["start"]["character"],
        11
    );
    assert_eq!(symbols["result"][1]["name"], "if");
    assert_eq!(symbols["result"][1]["children"][1]["name"], "else");

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":4,"method":"textDocument/completion","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":9}}}),
    )
    .await;
    let completion = receive(&mut stdout).await;
    assert_eq!(completion["id"], 4);
    assert!(completion["result"].is_array());
    assert_eq!(completion["result"][0]["label"], "name");
    assert_eq!(completion["result"][0]["kind"], 6);
    assert_eq!(completion["result"][0]["detail"], "parameter: String");

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":5,"method":"textDocument/hover","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":2}}}),
    )
    .await;
    let section_hover = receive(&mut stdout).await;
    assert_eq!(section_hover["id"], 5);
    assert_eq!(section_hover["result"]["contents"]["kind"], "markdown");
    assert_eq!(
        section_hover["result"]["contents"]["value"],
        "```radiant\n{#if condition}…{/if}\n```\n\nConditionally renders its body."
    );
    assert_eq!(
        section_hover["result"]["range"],
        json!({"start":{"line":1,"character":2},"end":{"line":1,"character":4}})
    );

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":6,"method":"textDocument/hover","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":11}}}),
    )
    .await;
    let declaration_hover = receive(&mut stdout).await;
    assert_eq!(declaration_hover["id"], 6);
    assert_eq!(
        declaration_hover["result"]["contents"]["value"],
        "**parameter** `name`\n\nType: `String`"
    );

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":7,"method":"textDocument/definition","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":11}}}),
    )
    .await;
    let definition = receive(&mut stdout).await;
    assert_eq!(definition["id"], 7);
    assert_eq!(definition["result"]["uri"], uri);
    assert_eq!(
        definition["result"]["range"],
        json!({"start":{"line":0,"character":11},"end":{"line":0,"character":15}})
    );

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":8,"method":"textDocument/definition","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":22}}}),
    )
    .await;
    let unknown_definition = receive(&mut stdout).await;
    assert_eq!(unknown_definition["id"], 8);
    assert!(unknown_definition["result"].is_null());

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":3},"contentChanges":[{"text":"{#i"}]}}),
    )
    .await;
    assert_eq!(receive(&mut stdout).await["params"]["version"], 3);
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":9,"method":"textDocument/completion","params":{"textDocument":{"uri":uri},"position":{"line":0,"character":3}}}),
    )
    .await;
    let plain_completion = receive(&mut stdout).await;
    assert_eq!(plain_completion["id"], 9);
    assert_eq!(
        plain_completion["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["label"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["if", "insert", "include"]
    );
    assert_eq!(
        plain_completion["result"][0]["insertText"],
        "if condition}{/if}"
    );
    assert!(plain_completion["result"][0]["insertTextFormat"].is_null());

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"{stale +}"}]}}),
    )
    .await;
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":uri}}}),
    )
    .await;
    let closed = receive(&mut stdout).await;
    assert_eq!(closed["params"]["uri"], uri);
    assert!(closed["params"]["version"].is_null());
    assert_eq!(closed["params"]["diagnostics"], json!([]));

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}),
    )
    .await;
    assert_eq!(receive(&mut stdout).await["id"], 2);
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    drop(stdin);
    assert!(child.wait().await.unwrap().success());
}

#[tokio::test]
async fn navigation_requests_respect_bindings_fragments_roots_and_open_overlays() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    for root in [&first, &second] {
        fs::create_dir_all(root.path().join("templates/tags")).unwrap();
    }
    let definitions_path = first.path().join("templates/defs.html");
    let layout_path = first.path().join("templates/layout.html");
    let tag_path = first.path().join("templates/tags/card.html");
    let other_path = second.path().join("templates/other.html");
    fs::write(&definitions_path, "{#fragment stale /}").unwrap();
    fs::write(&layout_path, "layout").unwrap();
    fs::write(&tag_path, "card").unwrap();
    fs::write(&other_path, "{#fragment Shared /}").unwrap();

    let definitions_uri = tower_lsp::lsp_types::Url::from_file_path(&definitions_path).unwrap();
    let page_uri =
        tower_lsp::lsp_types::Url::from_file_path(first.path().join("templates/page.html"))
            .unwrap();
    let first_uri = tower_lsp::lsp_types::Url::from_file_path(first.path()).unwrap();
    let second_uri = tower_lsp::lsp_types::Url::from_file_path(second.path()).unwrap();
    let definitions = "😀\n{#fragment Shared /}\n{#capture Note /}\n{#include $Shared /}";
    let page = concat!(
        "😀{@String item}{item}\n",
        "{#for item in items}{item}{#let item='x'}{item}{/let}{item}{/for}\n",
        "{item}\n",
        "{#include defs$Shared /}{#include defs$Note /}",
        "{#include layout /}{#include _id=layout /}{#card /}{#card /}"
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_radiant-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    send(
        &mut stdin,
        json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "workspaceFolders":[
                    {"uri":first_uri,"name":"first"},
                    {"uri":second_uri,"name":"second"}
                ],
                "capabilities":{}
            }
        }),
    )
    .await;
    assert_eq!(receive(&mut stdout).await["id"], 1);
    for (uri, text) in [(&definitions_uri, definitions), (&page_uri, page)] {
        send(
            &mut stdin,
            json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{
                "uri":uri,"languageId":"radiant","version":1,"text":text
            }}}),
        )
        .await;
        let diagnostics = receive_diagnostics(&mut stdout, uri.as_str()).await;
        assert_eq!(diagnostics["params"]["diagnostics"], json!([]));
    }

    let page_lines = radiant_lsp::LineIndex::new(page);
    let definitions_lines = radiant_lsp::LineIndex::new(definitions);
    let parameter_use = page.find("{item}").unwrap() + 1;
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/references","params":{
            "textDocument":{"uri":page_uri},
            "position":page_lines.byte_to_position(parameter_use),
            "context":{"includeDeclaration":true}
        }}),
    )
    .await;
    let parameter_references = receive_response(&mut stdout, 2).await;
    assert_eq!(parameter_references["result"].as_array().unwrap().len(), 3);
    assert!(
        parameter_references["result"]
            .as_array()
            .unwrap()
            .iter()
            .any(|location| {
                location["range"]
                    == json!(page_lines.span_to_range(radiant_compiler::Span::new(
                        page.find("item}").unwrap(),
                        page.find("item}").unwrap() + 4,
                    )))
            })
    );

    let local_use = page.find("{item}{/let}").unwrap() + 1;
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/references","params":{
            "textDocument":{"uri":page_uri},
            "position":page_lines.byte_to_position(local_use),
            "context":{"includeDeclaration":false}
        }}),
    )
    .await;
    let local_references = receive_response(&mut stdout, 3).await;
    assert_eq!(local_references["result"].as_array().unwrap().len(), 1);
    assert_eq!(
        local_references["result"][0]["range"],
        json!(page_lines.span_to_range(radiant_compiler::Span::new(local_use, local_use + 4)))
    );

    let shared_reference = page.find("defs$Shared").unwrap() + "defs$".len();
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":4,"method":"textDocument/definition","params":{
            "textDocument":{"uri":page_uri},
            "position":page_lines.byte_to_position(shared_reference)
        }}),
    )
    .await;
    let definition = receive_response(&mut stdout, 4).await;
    let shared_declaration = definitions.find("Shared").unwrap();
    assert_eq!(definition["result"]["uri"], definitions_uri.as_str());
    assert_eq!(
        definition["result"]["range"],
        json!(definitions_lines.span_to_range(radiant_compiler::Span::new(
            shared_declaration,
            shared_declaration + 6
        )))
    );

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":5,"method":"textDocument/references","params":{
            "textDocument":{"uri":definitions_uri},
            "position":definitions_lines.byte_to_position(shared_declaration),
            "context":{"includeDeclaration":true}
        }}),
    )
    .await;
    let fragment_references = receive_response(&mut stdout, 5).await;
    assert_eq!(fragment_references["result"].as_array().unwrap().len(), 3);
    assert!(
        fragment_references["result"]
            .as_array()
            .unwrap()
            .iter()
            .any(|location| {
                location["uri"] == definitions_uri.as_str()
                    && location["range"]["start"]
                        == json!(
                            definitions_lines
                                .byte_to_position(definitions.rfind("Shared").unwrap())
                        )
            })
    );

    let note_reference = page.find("defs$Note").unwrap() + "defs$".len();
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":6,"method":"textDocument/references","params":{
            "textDocument":{"uri":page_uri},
            "position":page_lines.byte_to_position(note_reference),
            "context":{"includeDeclaration":true}
        }}),
    )
    .await;
    let capture_references = receive_response(&mut stdout, 6).await;
    assert_eq!(capture_references["result"].as_array().unwrap().len(), 2);

    for (id, needle, expected_count) in [(7, "layout /}", 1), (8, "card /}", 2)] {
        let at = page.find(needle).unwrap();
        send(
            &mut stdin,
            json!({"jsonrpc":"2.0","id":id,"method":"textDocument/references","params":{
                "textDocument":{"uri":page_uri},
                "position":page_lines.byte_to_position(at),
                "context":{"includeDeclaration":false}
            }}),
        )
        .await;
        assert_eq!(
            receive_response(&mut stdout, id).await["result"]
                .as_array()
                .unwrap()
                .len(),
            expected_count
        );
    }

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":9,"method":"workspace/symbol","params":{"query":"shared"}}),
    )
    .await;
    let workspace_symbols = receive_response(&mut stdout, 9).await;
    assert_eq!(workspace_symbols["result"].as_array().unwrap().len(), 2);
    assert!(
        workspace_symbols["result"]
            .as_array()
            .unwrap()
            .iter()
            .any(|symbol| {
                symbol["location"]["uri"] == definitions_uri.as_str()
                    && symbol["location"]["range"]
                        == json!(definitions_lines.span_to_range(radiant_compiler::Span::new(
                            shared_declaration,
                            shared_declaration + 6,
                        )))
            })
    );
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":10,"method":"workspace/symbol","params":{"query":"stale"}}),
    )
    .await;
    assert_eq!(receive_response(&mut stdout, 10).await["result"], json!([]));
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":11,"method":"workspace/symbol","params":{"query":"defs"}}),
    )
    .await;
    let template_symbol = receive_response(&mut stdout, 11).await;
    assert_eq!(template_symbol["result"][0]["name"], "defs");
    assert_eq!(
        template_symbol["result"][0]["location"]["range"],
        json!(definitions_lines.span_to_range(radiant_compiler::Span::new(0, definitions.len())))
    );

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":12,"method":"shutdown","params":null}),
    )
    .await;
    assert_eq!(receive_response(&mut stdout, 12).await["id"], 12);
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    drop(stdin);
    assert!(child.wait().await.unwrap().success());
}

#[tokio::test]
async fn workspace_templates_register_watchers_refresh_and_return_protocol_shapes() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    let added = tempdir().unwrap();
    fs::create_dir_all(first.path().join("templates/layouts")).unwrap();
    fs::create_dir_all(first.path().join("templates/tags")).unwrap();
    fs::create_dir_all(first.path().join("templates/.hidden")).unwrap();
    fs::create_dir_all(second.path().join("templates")).unwrap();
    fs::create_dir_all(added.path().join("templates")).unwrap();
    let layout = first.path().join("templates/layouts/base.html");
    let tag = first.path().join("templates/tags/card.html");
    fs::write(
        &layout,
        "{#insert header}{/insert}{#insert body /}{#insert footer}{/insert}",
    )
    .unwrap();
    fs::write(&tag, "tag").unwrap();
    fs::write(
        first.path().join("templates/fragments.html"),
        "{#fragment primary /}{#capture private /}{#fragment secondary /}",
    )
    .unwrap();
    fs::write(second.path().join("templates/isolated.html"), "other").unwrap();
    fs::write(first.path().join("templates/.hidden/secret.html"), "hidden").unwrap();
    fs::write(first.path().join("templates/backup.html~"), "backup").unwrap();
    fs::write(added.path().join("templates/dynamic.html"), "dynamic").unwrap();
    let first_uri = tower_lsp::lsp_types::Url::from_file_path(first.path()).unwrap();
    let second_uri = tower_lsp::lsp_types::Url::from_file_path(second.path()).unwrap();
    let added_uri = tower_lsp::lsp_types::Url::from_file_path(added.path()).unwrap();
    let document_uri =
        tower_lsp::lsp_types::Url::from_file_path(first.path().join("templates/page.html"))
            .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_radiant-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    send(
        &mut stdin,
        json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{
                "rootUri":"file:///ignored-fallback",
                "workspaceFolders":[
                    {"uri":first_uri,"name":"first"},
                    {"uri":second_uri,"name":"second"}
                ],
                "capabilities":{
                    "workspace":{"didChangeWatchedFiles":{"dynamicRegistration":true}},
                    "textDocument":{"completion":{"completionItem":{"snippetSupport":true}}}
                }
            }
        }),
    )
    .await;
    let initialized = receive(&mut stdout).await;
    assert_eq!(initialized["id"], 1);
    assert_eq!(
        initialized["result"]["capabilities"]["workspace"]["workspaceFolders"],
        json!({"supported":true,"changeNotifications":true})
    );
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )
    .await;
    let registration = receive(&mut stdout).await;
    assert_eq!(registration["method"], "client/registerCapability");
    assert_eq!(
        registration["params"]["registrations"][0]["method"],
        "workspace/didChangeWatchedFiles"
    );
    assert_eq!(
        registration["params"]["registrations"][0]["registerOptions"]["watchers"][0]["globPattern"],
        "**/templates/**"
    );
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":registration["id"],"result":null}),
    )
    .await;
    assert_eq!(receive(&mut stdout).await["method"], "window/logMessage");

    let incomplete = "{#include lay";
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document_uri,"languageId":"radiant","version":1,"text":incomplete}}}),
    )
    .await;
    assert_eq!(
        receive(&mut stdout).await["method"],
        "textDocument/publishDiagnostics"
    );
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":document_uri},"position":{"line":0,"character":incomplete.len()}}}),
    )
    .await;
    let initial = receive(&mut stdout).await;
    assert_eq!(initial["id"], 2);
    let labels = initial["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(labels, ["layouts/base"]);
    assert_eq!(initial["result"][0]["kind"], 17);
    assert!(!labels.contains(&"isolated"));

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"workspace/didChangeWorkspaceFolders","params":{"event":{
            "added":[{"uri":added_uri,"name":"added"}],
            "removed":[{"uri":second_uri,"name":"second"}]
        }}}),
    )
    .await;
    assert_eq!(
        receive_diagnostics(&mut stdout, document_uri.as_str()).await["params"]["version"],
        1
    );
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":20,"method":"textDocument/completion","params":{"textDocument":{"uri":document_uri},"position":{"line":0,"character":incomplete.len()}}}),
    )
    .await;
    assert_eq!(
        receive(&mut stdout).await["result"][0]["label"],
        "layouts/base"
    );

    let added_document =
        tower_lsp::lsp_types::Url::from_file_path(added.path().join("templates/page.html"))
            .unwrap();
    let added_source = "{#include dyn";
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":added_document,"languageId":"radiant","version":1,"text":added_source}}}),
    )
    .await;
    let _ = receive_diagnostics(&mut stdout, added_document.as_str()).await;
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":21,"method":"textDocument/completion","params":{"textDocument":{"uri":added_document},"position":{"line":0,"character":added_source.len()}}}),
    )
    .await;
    assert_eq!(
        receive_response(&mut stdout, 21).await["result"][0]["label"],
        "dynamic"
    );

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"workspace/didChangeWorkspaceFolders","params":{"event":{
            "added":[],
            "removed":[{"uri":added_uri,"name":"added"}]
        }}}),
    )
    .await;
    for _ in 0..2 {
        assert_eq!(
            receive(&mut stdout).await["method"],
            "textDocument/publishDiagnostics"
        );
    }
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":22,"method":"textDocument/completion","params":{"textDocument":{"uri":added_document},"position":{"line":0,"character":added_source.len()}}}),
    )
    .await;
    assert_eq!(receive(&mut stdout).await["result"], json!([]));
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":added_document}}}),
    )
    .await;
    assert_eq!(
        receive(&mut stdout).await["method"],
        "textDocument/publishDiagnostics"
    );
    assert_eq!(
        receive(&mut stdout).await["method"],
        "textDocument/publishDiagnostics"
    );

    for (id, source, expected) in [
        (8, "{#i", vec!["if", "insert", "include"]),
        (9, "{#include fragments$pr", vec!["primary", "private"]),
        (10, "{#include layouts/base}{#he", vec!["header"]),
    ] {
        send(
            &mut stdin,
            json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document_uri,"version":id},"contentChanges":[{"text":source}]}}),
        )
        .await;
        assert_eq!(receive(&mut stdout).await["params"]["version"], id);
        send(
            &mut stdin,
            json!({"jsonrpc":"2.0","id":id,"method":"textDocument/completion","params":{"textDocument":{"uri":document_uri},"position":{"line":0,"character":source.len()}}}),
        )
        .await;
        let completion = receive(&mut stdout).await;
        assert_eq!(completion["id"], id);
        assert_eq!(
            completion["result"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["label"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        if id == 8 {
            assert_eq!(
                completion["result"][0]["insertText"],
                "if ${1:condition}}${0}{/if}"
            );
            assert_eq!(completion["result"][0]["insertTextFormat"], 2);
            assert_eq!(completion["result"][0]["kind"], 15);
        }
    }

    let all_templates = "{#include ";
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document_uri,"version":11},"contentChanges":[{"text":all_templates}]}}),
    )
    .await;
    assert_eq!(receive(&mut stdout).await["params"]["version"], 11);

    let created = first.path().join("templates/new.txt");
    fs::write(&created, "new").unwrap();
    let created_uri = tower_lsp::lsp_types::Url::from_file_path(&created).unwrap();
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":created_uri,"type":1}]}}),
    )
    .await;
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":created_uri,"type":2}]}}),
    )
    .await;
    let hidden_created = first.path().join("templates/.ignored.html");
    fs::write(&hidden_created, "ignored").unwrap();
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":tower_lsp::lsp_types::Url::from_file_path(hidden_created).unwrap(),"type":1}]}}),
    )
    .await;
    assert_eq!(
        receive_diagnostics(&mut stdout, document_uri.as_str()).await["params"]["version"],
        11
    );
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":document_uri},"position":{"line":0,"character":all_templates.len()}}}),
    )
    .await;
    let refreshed = receive(&mut stdout).await;
    let refreshed_labels = refreshed["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(refreshed_labels.contains(&"new"));
    assert!(!refreshed_labels.contains(&".ignored"));
    assert!(!refreshed_labels.contains(&".hidden/secret"));
    assert!(!refreshed_labels.contains(&"backup"));
    fs::remove_file(&created).unwrap();
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":created_uri,"type":3}]}}),
    )
    .await;
    assert_eq!(
        receive_diagnostics(&mut stdout, document_uri.as_str()).await["params"]["version"],
        11
    );

    let references = "{#include layouts/base /}{#card /}";
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document_uri,"version":12},"contentChanges":[{"text":references}]}}),
    )
    .await;
    assert_eq!(receive(&mut stdout).await["params"]["version"], 12);
    for (id, character, target) in [
        (4, references.find("layouts/base").unwrap(), &layout),
        (5, references.find("card").unwrap(), &tag),
    ] {
        send(
            &mut stdin,
            json!({"jsonrpc":"2.0","id":id,"method":"textDocument/definition","params":{"textDocument":{"uri":document_uri},"position":{"line":0,"character":character}}}),
        )
        .await;
        let definition = receive(&mut stdout).await;
        assert_eq!(definition["id"], id);
        assert_eq!(
            definition["result"]["uri"],
            json!(tower_lsp::lsp_types::Url::from_file_path(target).unwrap())
        );
        assert_eq!(
            definition["result"]["range"],
            json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}})
        );
    }

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document_uri,"version":13},"contentChanges":[{"text":all_templates}]}}),
    )
    .await;
    assert_eq!(receive(&mut stdout).await["params"]["version"], 13);
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":6,"method":"textDocument/completion","params":{"textDocument":{"uri":document_uri},"position":{"line":0,"character":all_templates.len()}}}),
    )
    .await;
    assert!(
        receive(&mut stdout).await["result"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["label"] != "new")
    );

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":7,"method":"shutdown","params":null}),
    )
    .await;
    assert_eq!(receive(&mut stdout).await["id"], 7);
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    drop(stdin);
    assert!(child.wait().await.unwrap().success());
}

#[tokio::test]
async fn root_uri_is_used_when_workspace_folders_are_absent() {
    let workspace = tempdir().unwrap();
    fs::create_dir_all(workspace.path().join("templates")).unwrap();
    fs::write(workspace.path().join("templates/fallback.html"), "fallback").unwrap();
    let root_uri = tower_lsp::lsp_types::Url::from_file_path(workspace.path()).unwrap();
    let document_uri =
        tower_lsp::lsp_types::Url::from_file_path(workspace.path().join("templates/page.html"))
            .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_radiant-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"rootUri":root_uri,"capabilities":{}}}),
    )
    .await;
    assert_eq!(receive(&mut stdout).await["id"], 1);
    let source = "{#include fall";
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document_uri,"languageId":"radiant","version":1,"text":source}}}),
    )
    .await;
    assert_eq!(
        receive(&mut stdout).await["method"],
        "textDocument/publishDiagnostics"
    );
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":document_uri},"position":{"line":0,"character":source.len()}}}),
    )
    .await;
    let completion = receive(&mut stdout).await;
    assert_eq!(completion["id"], 2);
    assert_eq!(completion["result"][0]["label"], "fallback");

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}),
    )
    .await;
    assert_eq!(receive(&mut stdout).await["id"], 3);
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    drop(stdin);
    assert!(child.wait().await.unwrap().success());
}

#[tokio::test]
async fn cross_template_diagnostics_use_open_overlays_and_refresh_watched_files() {
    let workspace = tempdir().unwrap();
    let templates = workspace.path().join("templates");
    fs::create_dir_all(&templates).unwrap();
    fs::write(templates.join("card.html"), "{#fragment present /}").unwrap();
    fs::write(templates.join("middle.html"), "{#include page /}").unwrap();
    let root_uri = tower_lsp::lsp_types::Url::from_file_path(workspace.path()).unwrap();
    let page = templates.join("page.html");
    let page_uri = tower_lsp::lsp_types::Url::from_file_path(&page).unwrap();
    let page_uri_text = page_uri.as_str();
    let source = concat!(
        "😀 {#include 'missing' /}\n",
        "{#lost /}\n",
        "{#include card$absent /}\n",
        "{#include _id=chosen /}\n",
        "{#include middle /}\n",
        "{broken +}"
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_radiant-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"rootUri":root_uri,"capabilities":{}}}),
    )
    .await;
    assert_eq!(receive(&mut stdout).await["id"], 1);
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":page_uri,"languageId":"radiant","version":1,"text":source}}}),
    )
    .await;
    let initial = receive_diagnostics(&mut stdout, page_uri_text).await;
    let diagnostics = initial["params"]["diagnostics"].as_array().unwrap();
    let codes = diagnostics
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        codes,
        [
            "E_EXPR_EXPECTED",
            "E_TEMPLATE_NOT_FOUND",
            "E_TAG_NOT_FOUND",
            "E_FRAGMENT_NOT_FOUND",
            "E_INCLUDE_CYCLE"
        ]
    );
    assert_eq!(
        diagnostics[1]["range"],
        json!({"start":{"line":0,"character":14},"end":{"line":0,"character":21}})
    );
    assert_eq!(
        diagnostics[2]["range"],
        json!({"start":{"line":1,"character":2},"end":{"line":1,"character":6}})
    );
    assert_eq!(
        diagnostics[3]["range"],
        json!({"start":{"line":2,"character":15},"end":{"line":2,"character":21}})
    );
    assert_eq!(
        diagnostics[4]["message"],
        "static include cycle: page -> middle -> page"
    );

    let missing = templates.join("missing.html");
    let tag = templates.join("tags/lost.html");
    fs::create_dir_all(tag.parent().unwrap()).unwrap();
    fs::write(&missing, "found").unwrap();
    fs::write(&tag, "found").unwrap();
    fs::write(templates.join("card.html"), "{#fragment absent /}").unwrap();
    fs::write(templates.join("middle.html"), "no cycle").unwrap();
    let changes = [
        &missing,
        &tag,
        &templates.join("card.html"),
        &templates.join("middle.html"),
    ]
    .into_iter()
    .map(|path| {
        json!({
            "uri":tower_lsp::lsp_types::Url::from_file_path(path).unwrap(),
            "type":2
        })
    })
    .collect::<Vec<_>>();
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":changes}}),
    )
    .await;
    let refreshed = receive_diagnostics(&mut stdout, page_uri_text).await;
    assert_eq!(
        refreshed["params"]["diagnostics"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        refreshed["params"]["diagnostics"][0]["code"],
        "E_EXPR_EXPECTED"
    );

    let fragment_source = "{#include card$future /}";
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":page_uri,"version":2},"contentChanges":[{"text":fragment_source}]}}),
    )
    .await;
    assert_eq!(
        receive_diagnostics(&mut stdout, page_uri_text).await["params"]["diagnostics"][0]["code"],
        "E_FRAGMENT_NOT_FOUND"
    );

    let card_uri = tower_lsp::lsp_types::Url::from_file_path(templates.join("card.html")).unwrap();
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":card_uri,"languageId":"radiant","version":1,"text":"{#fragment future /}"}}}),
    )
    .await;
    assert_eq!(
        receive_diagnostics(&mut stdout, page_uri_text).await["params"]["diagnostics"],
        json!([])
    );
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":card_uri,"version":2},"contentChanges":[{"text":"no fragments"}]}}),
    )
    .await;
    assert_eq!(
        receive_diagnostics(&mut stdout, page_uri_text).await["params"]["diagnostics"][0]["code"],
        "E_FRAGMENT_NOT_FOUND"
    );

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}),
    )
    .await;
    loop {
        let message = receive(&mut stdout).await;
        if message["id"] == 2 {
            break;
        }
    }
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    drop(stdin);
    assert!(child.wait().await.unwrap().success());
}
