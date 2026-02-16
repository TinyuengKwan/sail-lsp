# Sail LSP

A comprehensive Language Server Protocol (LSP) implementation for the [Sail ISA specification language](https://github.com/rems-project/sail), written in Rust.

## Features

This LSP server integrates directly with the Sail REPL to provide accurate, compiler-verified information.

- **Diagnostics**: Real-time syntax and semantic error reporting.
- **Go to Definition**: Jump to definitions of functions, types, registers, and variables across the project.
- **Hover**: View type signatures and documentation for symbols.
- **Completion**: Smart autocompletion for keywords, types, and defined symbols.
- **Document Symbols**: Outline view of symbols within the current file.
- **Workspace Symbols**: Project-wide symbol search.
- **References**: Find all usages of a symbol across the project.
- **Rename**: Project-wide symbol renaming.
- **Formatting**: Integrated code formatting using `sail --fmt`.

## Installation

### Prerequisites

- **Sail**: Ensure `sail` is installed and available in your PATH.
- **Rust**: You need the Rust toolchain (`cargo`) to build the project.

### Building

```bash
git clone https://github.com/TinyuengKwan/sail-lsp.git
cd sail-lsp
cargo build --release
```

The binary will be located at `target/release/sail-lsp`.

## Editor Configuration

### VS Code

Install the [Executable LSP link](https://marketplace.visualstudio.com/items?itemName=albert.TabNine) or configure a generic LSP client with the following settings:

```json
{
    "lsp-server.path": "/path/to/sail-lsp",
    "lsp-server.args": ["--sail-path", "sail"]
}
```

### Neovim (using nvim-lspconfig)

```lua
local configs = require 'lspconfig.configs'
if not configs.sail_lsp then
  configs.sail_lsp = {
    default_config = {
      cmd = {'sail-lsp'},
      filetypes = {'sail'},
      root_dir = require('lspconfig.util').root_pattern('ROOT', '.sail_project', '.git'),
      settings = {},
    },
  }
end
require('lspconfig').sail_lsp.setup{}
```

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
