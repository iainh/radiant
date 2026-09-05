#[tokio::main]
async fn main() {
    radiant_lsp::serve(tokio::io::stdin(), tokio::io::stdout()).await;
}
