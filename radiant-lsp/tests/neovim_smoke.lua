local server = vim.env.RADIANT_LSP
assert(server and server ~= "", "RADIANT_LSP must point to the radiant-lsp binary")
assert(vim.fn.executable(server) == 1, "RADIANT_LSP is not executable: " .. server)

local tree_sitter_parser = vim.env.RADIANT_TS_PARSER
assert(tree_sitter_parser and tree_sitter_parser ~= "",
  "RADIANT_TS_PARSER must point to the compiled Radiant parser")
assert(vim.fn.filereadable(tree_sitter_parser) == 1,
  "Radiant Tree-sitter parser is not readable: " .. tree_sitter_parser)

local grammar_root = vim.env.RADIANT_TS_ROOT or (vim.fn.getcwd() .. "/tree-sitter-radiant")
local highlight_path = grammar_root .. "/queries/highlights.scm"
local injection_path = grammar_root .. "/queries/injections.scm"
assert(vim.fn.filereadable(highlight_path) == 1,
  "Radiant Tree-sitter highlight query is not readable: " .. highlight_path)
assert(vim.fn.filereadable(injection_path) == 1,
  "Radiant Tree-sitter injection query is not readable: " .. injection_path)

vim.treesitter.language.add("radiant", { path = tree_sitter_parser })
local language = vim.treesitter.language.inspect("radiant")
assert(language and language.abi_version == 15,
  "Neovim did not load the ABI 15 Radiant Tree-sitter parser")
local highlight_source = table.concat(vim.fn.readfile(highlight_path), "\n")
local highlight_query = vim.treesitter.query.parse("radiant", highlight_source)
vim.treesitter.query.set("radiant", "highlights", highlight_source)
local injection_source = table.concat(vim.fn.readfile(injection_path), "\n")
vim.treesitter.query.set("radiant", "injections", injection_source)

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
  "<main class=\"card\"><!-- html -->{! note !}{| raw {value} |}<b>{name ?: 'guest'}</b></main>",
}
vim.fn.writefile(initial, page)
vim.cmd.edit(vim.fn.fnameescape(page))
local buffer = vim.api.nvim_get_current_buf()
vim.bo[buffer].filetype = "radiant"

local parser = vim.treesitter.get_parser(buffer, "radiant", { error = false })
assert(parser, "Neovim did not create a Radiant Tree-sitter parser")
local tree = assert(parser:parse()[1], "Neovim did not parse the Radiant template")
local captures = {}
for id, node in highlight_query:iter_captures(tree:root(), buffer, 0, -1) do
  local capture = highlight_query.captures[id]
  local text = vim.treesitter.get_node_text(node, buffer)
  captures[capture] = captures[capture] or {}
  captures[capture][text] = true
end

local function assert_capture(capture, text)
  assert(captures[capture] and captures[capture][text],
    string.format("missing Tree-sitter capture @%s for %q", capture, text))
end

assert_capture("tag", "main")
assert_capture("tag.attribute", "class")
assert_capture("type", "String")
assert_capture("variable.parameter", "name")
assert_capture("keyword", "if")
assert_capture("variable", "name")
assert_capture("operator", "?:")
assert_capture("string", "'guest'")
assert_capture("comment", "<!-- html -->")
assert_capture("comment", "{! note !}")
assert_capture("string.special", " raw {value} ")
vim.treesitter.start(buffer, "radiant")

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
  },
})
client:notify("workspace/didChangeWatchedFiles", {
  changes = {
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

local fragment_definition = request("textDocument/definition", {
  textDocument = text_document(),
  position = { line = 7, character = 20 },
})
assert(vim.uri_to_fname(fragment_definition.uri) == templates .. "/fragments.html",
  "fragment definition targeted the wrong template")
assert(fragment_definition.range.start.line == 0 and fragment_definition.range.start.character == 52,
  "fragment definition did not target the exact declaration name")
assert(fragment_definition.range["end"].line == 0 and fragment_definition.range["end"].character == 58,
  "fragment definition declaration range had the wrong end")

local local_references = request("textDocument/references", {
  textDocument = text_document(),
  position = { line = 3, character = 12 },
  context = { includeDeclaration = true },
})
assert(#local_references == 4, "parameter references did not include three uses and the declaration")
local saw_parameter_declaration = false
for _, location in ipairs(local_references) do
  if location.range.start.line == 0 and location.range.start.character == 11
      and location.range["end"].character == 15 then
    saw_parameter_declaration = true
  end
end
assert(saw_parameter_declaration, "parameter references omitted the exact declaration range")

local fragment_references = request("textDocument/references", {
  textDocument = text_document(),
  position = { line = 7, character = 20 },
  context = { includeDeclaration = true },
})
assert(#fragment_references == 2, "fragment references did not include its use and declaration")
assert(vim.uri_to_fname(fragment_references[1].uri) == templates .. "/fragments.html"
    and fragment_references[1].range.start.character == 52,
  "fragment references omitted the exact declaration")

local fragment_symbols = request("workspace/symbol", { query = "absent" })
assert(#fragment_symbols == 1 and fragment_symbols[1].name == "absent",
  "workspace symbol filtering did not return the fragment")
assert(vim.uri_to_fname(fragment_symbols[1].location.uri) == templates .. "/fragments.html"
    and fragment_symbols[1].location.range.start.character == 52,
  "workspace fragment symbol had the wrong exact location")

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

local added_root = vim.fn.tempname()
local added_templates = added_root .. "/templates"
vim.fn.mkdir(added_templates, "p")
vim.fn.writefile({ "dynamic" }, added_templates .. "/dynamic.html")
local added_page = added_templates .. "/page.html"
vim.fn.writefile({ "{#include dyn" }, added_page)
client:notify("workspace/didChangeWorkspaceFolders", {
  event = {
    added = { { uri = vim.uri_from_fname(added_root), name = "added" } },
    removed = {},
  },
})
local added_buffer = vim.fn.bufadd(added_page)
vim.fn.bufload(added_buffer)
vim.bo[added_buffer].filetype = "radiant"
assert(vim.lsp.buf_attach_client(added_buffer, client_id), "could not attach added workspace buffer")
assert(vim.wait(5000, function()
  local response = client:request_sync("textDocument/completion", {
    textDocument = { uri = vim.uri_from_bufnr(added_buffer) },
    position = { line = 0, character = 13 },
  }, 1000, added_buffer)
  return response and not response.err and response.result and response.result[1]
    and response.result[1].label == "dynamic"
end, 20), "added workspace folder was not indexed")

local added_symbols = request("workspace/symbol", { query = "dynamic" })
assert(#added_symbols == 1 and added_symbols[1].name == "dynamic",
  "workspace symbols did not include the second workspace root")
assert(vim.uri_to_fname(added_symbols[1].location.uri) == added_templates .. "/dynamic.html"
    and added_symbols[1].location.range.start.line == 0
    and added_symbols[1].location.range.start.character == 0
    and added_symbols[1].location.range["end"].line == 1
    and added_symbols[1].location.range["end"].character == 0,
  "second-root template symbol had the wrong exact location")

client:notify("workspace/didChangeWorkspaceFolders", {
  event = {
    added = {},
    removed = { { uri = vim.uri_from_fname(added_root), name = "added" } },
  },
})
assert(vim.wait(5000, function()
  local response = client:request_sync("textDocument/completion", {
    textDocument = { uri = vim.uri_from_bufnr(added_buffer) },
    position = { line = 0, character = 13 },
  }, 1000, added_buffer)
  return response and not response.err and response.result and #response.result == 0
end, 20), "removed workspace folder remained indexed or its open document stopped serving requests")

client:stop()
assert(vim.wait(5000, function()
  return client:is_stopped()
end, 20), "radiant-lsp did not stop cleanly")
vim.fn.delete(root, "rf")
vim.fn.delete(added_root, "rf")
print("radiant Neovim 0.11.4 acceptance: compiled Tree-sitter parser and HTML/Radiant captures; dynamic workspace add/remove with open-document service; debounced watched-file bursts and diagnostic clearing; exact definitions, references and multi-root workspace symbols; hover, typed completion order, negotiated snippets, fragments/captures and layout blocks passed")
