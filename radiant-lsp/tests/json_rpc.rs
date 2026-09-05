use std::{process::Stdio, time::Duration};

use serde_json::{Value, json};
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
