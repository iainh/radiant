local server = vim.env.RADIANT_LSP
assert(server and server ~= "", "RADIANT_LSP must point to the radiant-lsp binary")
assert(vim.fn.executable(server) == 1, "RADIANT_LSP is not executable: " .. server)

local root = vim.fn.tempname()
local templates = root .. "/templates"
vim.fn.mkdir(templates .. "/layouts", "p")
vim.fn.mkdir(templates .. "/tags", "p")
vim.fn.writefile({ "<main>{#insert body /}</main>" }, templates .. "/layouts/base.html")
vim.fn.writefile({ "<article>{#nested-content /}</article>" }, templates .. "/tags/card.html")

local page = templates .. "/page.html"
local initial = {
  "😀{@String name}",
  "{#include layouts/base /}",
  "{#card /}",
  "{#if name}{name}{/if}",
  "{broken +}",
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
assert(diagnostics[1].source == "radiant", "diagnostic source was not Radiant")
assert(diagnostics[1].code == "E_EXPR_EXPECTED", "unexpected diagnostic code")

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

local function completion_labels(position)
  local completion = request("textDocument/completion", {
    textDocument = text_document(),
    position = position,
  })
  local labels = {}
  local items = completion.items or completion
  for _, item in ipairs(items) do
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

client:stop()
assert(vim.wait(5000, function()
  return client:is_stopped()
end, 20), "radiant-lsp did not stop cleanly")
vim.fn.delete(root, "rf")
print("radiant-lsp Neovim acceptance: diagnostics, symbols, completion, hover and definitions passed")
