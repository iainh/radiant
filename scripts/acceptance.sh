#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

for command in cargo cc diff node npm nvim; do
    command -v "$command" >/dev/null || {
        printf 'error: required command not found: %s\n' "$command" >&2
        exit 1
    }
done

node_version=$(<.node-version)
if [[ $(node --version) != "v$node_version" ]]; then
    printf 'error: Node.js %s is required\n' "$node_version" >&2
    exit 1
fi

if [[ $(nvim --version | head -n 1) != "NVIM v0.11.4" ]]; then
    printf 'error: Neovim 0.11.4 is required\n' >&2
    exit 1
fi

cargo fmt --all -- --check
cargo build --locked --workspace --all-features --all-targets
cargo test --locked --workspace --all-features
cargo clippy --locked --workspace --all-features --all-targets -- -D warnings

generated_snapshot=$(mktemp -d)
trap 'rm -rf "$generated_snapshot"' EXIT
cp -a tree-sitter-radiant/src "$generated_snapshot/src"
(
    cd tree-sitter-radiant
    npm ci
    npm run generate
    npm test
)
diff --recursive --unified \
    --exclude=grammar.json --exclude=node-types.json \
    "$generated_snapshot/src" tree-sitter-radiant/src

mkdir -p target/tree-sitter
cc -O2 -fPIC -shared -I tree-sitter-radiant/src \
    tree-sitter-radiant/src/parser.c tree-sitter-radiant/src/scanner.c \
    -o target/tree-sitter/radiant.so

RADIANT_LSP="$repo_root/target/debug/radiant-lsp" \
RADIANT_TS_PARSER="$repo_root/target/tree-sitter/radiant.so" \
RADIANT_TS_ROOT="$repo_root/tree-sitter-radiant" \
    nvim -u NONE --noplugin -i NONE --headless -l radiant-lsp/tests/neovim_smoke.lua
