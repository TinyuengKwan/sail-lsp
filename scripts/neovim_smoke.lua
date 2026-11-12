-- Run with Neovim >= 0.11: SAIL_LSP_TEST_SERVER=/absolute/sail-lsp nvim --headless -u NONE -l scripts/neovim_smoke.lua
-- Uses the installed template and an actual server, not a mocked LSP API.
local root = vim.fn.tempname()
local client
local function run()
  local repo = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":p:h:h")
  local server = assert(vim.env.SAIL_LSP_TEST_SERVER, "SAIL_LSP_TEST_SERVER is required")
  vim.fn.mkdir(root .. "/empty-sail/lib", "p")
  vim.env.SAIL_DIR = root .. "/empty-sail"
  vim.fn.mkdir(root .. "/nested", "p")
  vim.fn.writefile({ "Main { files main.sail }" }, root .. "/model.sail_project")
  vim.fn.writefile({ "val identity : int -> int", '$include "nested/child.sail"', "$define SECOND", '$include "nested/child.sail"' }, root .. "/main.sail")
  vim.fn.writefile({ "$ifdef SECOND", "function second() -> int = identity(true)", "$else", "function first() -> int = 1", "$endif" }, root .. "/nested/child.sail")
  local template = table.concat(vim.fn.readfile(repo .. "/editors/templates/neovim.lua"), "\n")
  template = template:gsub("@SERVER_JSON@", function() return vim.json.encode(server) end)
  assert(loadstring(template))()
  local status, status_updates = nil, 0
  vim.lsp.config("sail_lsp_managed", {
    capabilities = { experimental = { serverStatusNotification = true } },
    handlers = {
      ["sail-lsp/serverStatus"] = function(_, value)
        status, status_updates = value, status_updates + 1
      end,
    },
  })
  vim.cmd("filetype plugin on")
  vim.cmd.cd(vim.fn.fnameescape(root))
  vim.cmd.edit(vim.fn.fnameescape(root .. "/nested/child.sail"))
  local source_buffer = vim.api.nvim_get_current_buf()
  local function wait(label, predicate)
    assert(vim.wait(20000, predicate, 50), label)
  end
  wait("Sail client attaches", function()
    client = vim.lsp.get_clients({bufnr=source_buffer,name="sail_lsp_managed"})[1]
    return client and client.initialized
  end)
  local function request(method, params, buffer)
    local response = client:request_sync(method, params, 5000, buffer or source_buffer)
    assert(response and not response.err, vim.inspect(response))
    return response.result
  end
  local document = {uri=vim.uri_from_bufnr(source_buffer)}
  wait("two actual include contexts", function()
    return #request("sail-lsp/includeContexts", document) == 2
  end)
  vim.ui.select = function(items, _, callback) callback(items[#items]) end
  vim.api.nvim_win_set_cursor(0,{2,37})
  vim.cmd.SailContext()
  wait("context selection", function() return request("sail-lsp/includeContexts",document)[2].selected end)
  local function preview(command, expected)
    vim.api.nvim_set_current_buf(source_buffer)
    vim.api.nvim_win_set_cursor(0,{2,37})
    vim.cmd(command)
    wait(command .. " opens a preview",function()return vim.api.nvim_buf_get_name(0):match("%.sail%-preview$") ~= nil end)
    local buffer = vim.api.nvim_get_current_buf()
    local text = table.concat(vim.api.nvim_buf_get_lines(buffer,0,-1,false),"\n")
    assert(text:find(expected,1,true),text)
    assert(vim.bo[buffer].readonly and not vim.bo[buffer].modifiable)
    wait("preview uses its originating client",function()return #vim.lsp.get_clients({bufnr=buffer,name="sail_lsp_managed"}) == 1 end)
    local links = request("textDocument/documentLink",{textDocument={uri=vim.uri_from_bufnr(buffer)}},buffer)
    assert(#links > 0)
    vim.api.nvim_win_set_cursor(0,{links[1].range.start.line+1,links[1].range.start.character})
    vim.cmd.SailSource()
    wait("preview source link",function()return vim.api.nvim_buf_get_name(0) ~= vim.api.nvim_buf_get_name(buffer) end)
    assert(vim.api.nvim_buf_get_name(0):match("%.sail$"))
    return text
  end
  local expanded = preview("SailPreprocess","function second")
  assert(not expanded:find("function first",1,true),expanded)
  preview("SailExplain","Expected: int")
  vim.api.nvim_set_current_buf(source_buffer)
  vim.cmd.SailFormatInfo()
  wait("formatting explanation",function()return vim.api.nvim_buf_get_name(0):match("%.sail%-preview$") ~= nil end)
  local formatting_info = vim.json.decode(table.concat(vim.api.nvim_buf_get_lines(0,0,-1,false),"\n"))
  assert(formatting_info.styleVersion == 1 and formatting_info.options.tabSize == 2)
  assert(vim.bo.readonly and not vim.bo.modifiable)
  vim.api.nvim_set_current_buf(source_buffer)
  vim.cmd.SailProject()
  wait("manifest navigation",function()return vim.api.nvim_buf_get_name(0) == root .. "/model.sail_project" end)
  vim.api.nvim_set_current_buf(source_buffer)
  local previous_status_updates = status_updates
  vim.cmd.SailSelectProject()
  wait("project selection",function()return client.settings["sail-lsp"].workspace and client.settings["sail-lsp"].workspace.activeProject ~= nil end)
  wait("selected project reload", function()
    return status_updates > previous_status_updates and status and status.quiescent
  end)
  local discovered = false
  vim.ui.select = function(items, _, callback)
    for _,item in ipairs(items) do if item.action and item.action.title:find("Check",1,true) then discovered = true end end
    callback(nil)
  end
  vim.cmd.SailTasks()
  wait("project task menu",function()return discovered end)
  print("Neovim: project/include selection, previews, explanation, source links and task discovery passed")
end
local ok, err = xpcall(run,debug.traceback)
if client then client:stop(); vim.wait(5000,function()return client:is_stopped() end,50) end
vim.fn.delete(root,"rf")
if not ok then io.stderr:write(err .. "\n"); vim.cmd("cquit 1") end
vim.cmd("qa!")
