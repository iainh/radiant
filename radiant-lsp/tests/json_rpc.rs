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
async fn workspace_templates_register_watchers_refresh_and_return_protocol_shapes() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    fs::create_dir_all(first.path().join("templates/layouts")).unwrap();
    fs::create_dir_all(first.path().join("templates/tags")).unwrap();
    fs::create_dir_all(second.path().join("templates")).unwrap();
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
    let first_uri = tower_lsp::lsp_types::Url::from_file_path(first.path()).unwrap();
    let second_uri = tower_lsp::lsp_types::Url::from_file_path(second.path()).unwrap();
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
    assert_eq!(receive(&mut stdout).await["id"], 1);
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
    assert!(
        refreshed["result"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "new")
    );
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
