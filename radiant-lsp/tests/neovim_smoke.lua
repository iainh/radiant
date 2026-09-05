local server = vim.env.RADIANT_LSP
assert(server and server ~= "", "RADIANT_LSP must point to the radiant-lsp binary")
assert(vim.fn.executable(server) == 1, "RADIANT_LSP is not executable: " .. server)

local root = vim.fn.tempname()
local templates = root .. "/templates"
vim.fn.mkdir(templates .. "/layouts", "p")
vim.fn.mkdir(templates .. "/tags", "p")
vim.fn.writefile({ "<main>{#insert header}{/insert}{#insert body /}{#insert footer}{/insert}</main>" }, templates .. "/layouts/base.html")
vim.fn.writefile({ "<article>{#nested-content /}</article>" }, templates .. "/tags/card.html")
vim.fn.writefile({ "{#fragment present /}" }, templates .. "/fragments.html")
vim.fn.writefile({ "{#include page /}" }, templates .. "/cycle.html")

local page = templates .. "/page.html"
local initial = {
  "😀{@String name}",
  "{#include layouts/base /}",
  "{#card /}",
  "{#if name}{name}{/if}",
  "{broken +}",
  "{#include missing /}",
  "{#lost /}",
  "{#include fragments$absent /}",
  "{#include cycle /}",
  "{#include _id=chosen /}",
}
vim.fn.writefile(initial, page)
vim.cmd.edit(vim.fn.fnameescape(page))
local buffer = vim.api.nvim_get_current_buf()
vim.bo[buffer].filetype = "radiant"

local client_id = vim.lsp.start({
  name = "radiant-editor-smoke",
  cmd = { server },
  root_dir = root,
})
assert(client_id, "Neovim did not start radiant-lsp")
local client = assert(vim.lsp.get_client_by_id(client_id), "radiant-lsp client is unavailable")
assert(vim.wait(5000, function()
  return client.initialized
end, 20), "radiant-lsp did not initialize")
assert(vim.wait(5000, function()
  return #vim.diagnostic.get(buffer) > 0
end, 20), "Neovim did not receive Radiant diagnostics")

local diagnostics = vim.diagnostic.get(buffer)
local diagnostic_codes = {}
for _, diagnostic in ipairs(diagnostics) do
  assert(diagnostic.source == "radiant", "diagnostic source was not Radiant")
  diagnostic_codes[diagnostic.code] = true
end
for _, code in ipairs({
  "E_EXPR_EXPECTED",
  "E_TEMPLATE_NOT_FOUND",
  "E_TAG_NOT_FOUND",
  "E_FRAGMENT_NOT_FOUND",
  "E_INCLUDE_CYCLE",
}) do
  assert(diagnostic_codes[code], "missing representative cross-template diagnostic " .. code)
end

vim.fn.mkdir(templates .. "/tags", "p")
vim.fn.writefile({ "created" }, templates .. "/missing.html")
vim.fn.writefile({ "created" }, templates .. "/tags/lost.html")
vim.fn.writefile({ "{#fragment present /}{#capture private /}{#fragment absent /}" }, templates .. "/fragments.html")
vim.fn.writefile({ "cycle fixed" }, templates .. "/cycle.html")
client:notify("workspace/didChangeWatchedFiles", {
  changes = {
    { uri = vim.uri_from_fname(templates .. "/missing.html"), type = 1 },
    { uri = vim.uri_from_fname(templates .. "/tags/lost.html"), type = 1 },
    { uri = vim.uri_from_fname(templates .. "/fragments.html"), type = 2 },
    { uri = vim.uri_from_fname(templates .. "/cycle.html"), type = 2 },
  },
})
assert(vim.wait(5000, function()
  local current = vim.diagnostic.get(buffer)
  return #current == 1 and current[1].code == "E_EXPR_EXPECTED"
end, 20), "creating and fixing referenced templates did not clear cross-template diagnostics")

local function text_document()
  return { uri = vim.uri_from_bufnr(buffer) }
end

local function request(method, params)
  local response = client:request_sync(method, params, 5000, buffer)
  assert(response, method .. " timed out")
  assert(not response.err, method .. " failed: " .. vim.inspect(response.err))
  return response.result
end

local symbols = request("textDocument/documentSymbol", { textDocument = text_document() })
assert(#symbols >= 4, "document symbols were incomplete")
assert(symbols[1].name == "name", "parameter symbol was not returned")
assert(symbols[4].name == "if", "section symbol was not returned")

local hover = request("textDocument/hover", {
  textDocument = text_document(),
  position = { line = 3, character = 2 },
})
assert(hover.contents.kind == "markdown", "hover was not Markdown")
assert(hover.contents.value:find("Conditionally renders", 1, true), "built-in hover was missing")

local local_definition = request("textDocument/definition", {
  textDocument = text_document(),
  position = { line = 3, character = 12 },
})
assert(local_definition.uri == vim.uri_from_bufnr(buffer), "local definition targeted another file")
assert(local_definition.range.start.line == 0, "local definition did not target the parameter")

local include_definition = request("textDocument/definition", {
  textDocument = text_document(),
  position = { line = 1, character = 12 },
})
assert(vim.uri_to_fname(include_definition.uri) == templates .. "/layouts/base.html", "include definition targeted the wrong template")

local tag_definition = request("textDocument/definition", {
  textDocument = text_document(),
  position = { line = 2, character = 3 },
})
assert(vim.uri_to_fname(tag_definition.uri) == templates .. "/tags/card.html", "tag definition targeted the wrong template")

local function replace(lines)
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, lines)
  assert(vim.wait(5000, function()
    return vim.lsp.util.buf_versions[buffer] == vim.api.nvim_buf_get_changedtick(buffer)
  end, 20), "Neovim did not synchronize the changed buffer")
end

local function completion_items(position)
  local completion = request("textDocument/completion", {
    textDocument = text_document(),
    position = position,
  })
  return completion.items or completion
end

local function completion_labels(position)
  local labels = {}
  for _, item in ipairs(completion_items(position)) do
    labels[item.label] = true
  end
  return labels
end

replace({ "{#include " })
assert(completion_labels({ line = 0, character = 10 })["layouts/base"], "include completion was missing")

replace({ "{#" })
local sections = completion_labels({ line = 0, character = 2 })
assert(sections["if"], "built-in section completion was missing")
assert(sections.card, "user-tag completion was missing")

replace({ "{#i" })
local filtered = completion_items({ line = 0, character = 3 })
assert(#filtered == 3, "typed section prefix was not filtered server-side")
assert(filtered[1].label == "if" and filtered[2].label == "insert" and filtered[3].label == "include",
  "typed section completion relevance order was not deterministic: " .. vim.inspect(filtered))
assert(filtered[1].kind == vim.lsp.protocol.CompletionItemKind.Snippet, "built-in section was not a snippet item")
assert(filtered[1].insertText == "if ${1:condition}}${0}{/if}", "if snippet shape was incorrect")
assert(filtered[1].insertTextFormat == vim.lsp.protocol.InsertTextFormat.Snippet, "snippet insertion format was not advertised")

replace({ "{#n" })
local nested = completion_items({ line = 0, character = 3 })
assert(#nested == 1 and nested[1].label == "nested-content", "self-closing completion was missing")
assert(nested[1].insertText == "nested-content /}" and not nested[1].insertText:find("{/", 1, true),
  "self-closing completion incorrectly added a closing tag")

replace({ "{#include fragments$p" })
local fragments = completion_items({ line = 0, character = 21 })
assert(#fragments == 2 and fragments[1].label == "present" and fragments[2].label == "private",
  "fragment/capture completions were missing or unordered: " .. vim.inspect(fragments))

replace({ "{#include layouts/base}{#b" })
local layout_blocks = completion_items({ line = 0, character = 26 })
assert(#layout_blocks == 1 and layout_blocks[1].label == "body",
  "included layout block completion was missing: " .. vim.inspect(layout_blocks))

client:stop()
assert(vim.wait(5000, function()
  return client:is_stopped()
end, 20), "radiant-lsp did not stop cleanly")
vim.fn.delete(root, "rf")
print("radiant-lsp Neovim 0.11.4 acceptance: diagnostics/watchers, symbols, hover, definitions, typed completion order, negotiated snippets, fragments/captures and layout blocks passed")
