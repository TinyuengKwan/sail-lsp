## Projects

A `sail.proj` or a single `*.sail_project` at the workspace root is loaded on its own. With several, set `sail-lsp.workspace.projectFile` or list them in `sail-lsp.workspace.linkedProjects`; `sail-lsp.workspace.projectModules` selects root modules and `projectVariables` overrides manifest variables.

**Sail: Open Project Manifest** jumps from a source file to its manifest; **Sail: Show Module Dependencies** draws the module graph.
