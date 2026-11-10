# Installing Sail LSP without an extension marketplace

The repository contains the installers and release automation described here.
Until a maintainer publishes the first release using this workflow, the prebuilt
commands cannot download these new assets. A checkout can already install from
source. No release is published by running the local installer.

## Quick start

Linux or macOS, once the standalone release assets are published:

```sh
curl --proto '=https' --tlsv1.2 -sSf \
  https://raw.githubusercontent.com/TinyuengKwan/sail-lsp/main/install.sh | sh
```

Native Windows PowerShell:

```powershell
$installer = Join-Path $env:TEMP 'sail-lsp-install.ps1'
Invoke-WebRequest https://raw.githubusercontent.com/TinyuengKwan/sail-lsp/main/install.ps1 -OutFile $installer
& $installer
```

For reproducible installation, download the script from the chosen release tag
rather than `main`, inspect it, and pass `--version vX.Y.Z`. SHA256 sidecars detect
corrupted or mismatched downloads; they are served by the same release host and
are not independent signatures. `latest` means GitHub's latest stable release;
select prerelease tags explicitly.

Prebuilt installation needs no Rust, Node.js, Python, upstream Sail compiler or
system Z3. Unix needs a POSIX shell, curl for online installation, tar and a SHA256
utility (`sha256sum`, `shasum` or `openssl`). Linux release builds bundle the C++
standard library but still need glibc and libgcc_s; these are native binaries,
not fully static Linux executables. Windows uses the
native ZIP APIs and statically linked MSVC runtime. The standard library and Z3
are embedded. Upstream `sail` is optional and needed only for Flycheck.

The default installation is per user and needs no administrator privileges:

| Purpose | Unix | Windows |
| --- | --- | --- |
| Installation root | `${XDG_DATA_HOME:-~/.local/share}/sail-lsp` | `%LOCALAPPDATA%/sail-lsp` |
| Stable executable | `<root>/bin/sail-lsp` | `<root>/bin/sail-lsp.exe` |
| Integration registry | `${XDG_CONFIG_HOME:-~/.config}/sail-lsp` | `%APPDATA%/sail-lsp` |

Override the roots with `SAIL_LSP_HOME` / `SAIL_LSP_CONFIG_HOME`, or with
`--prefix DIR` / `--config-home DIR`. With a custom `--config-home`, also set
`SAIL_LSP_CONFIG_HOME` in the editor's environment so it finds that registry.
Unix PATH changes use marked blocks in the
current shell's startup files (or `--profile FILE`); fish uses its own `conf.d`
file. Windows changes only the user PATH. Open a new terminal afterward. Use
`--no-modify-path` to manage PATH yourself. Existing terminals and editor
processes keep their environment and running server until restarted.

## Source installation

Use the same entry point for a local checkout:

```sh
git clone https://github.com/TinyuengKwan/sail-lsp.git
cd sail-lsp
sh install.sh --from-source
# Include uncommitted local changes explicitly:
sh install.sh --path "$PWD"
# Require an exact, clean checkout:
sh install.sh --path "$PWD" --rev FULL_COMMIT_SHA
```

On Windows replace `sh install.sh` with `& .\install.ps1`. Source builds require
Rust/Cargo, Git when fetching sources, CMake and a C++20 compiler with working
`std::format` support (for example GCC 13 or newer); use the MSVC C++ build tools
on Windows. Ubuntu 22.04's default GCC 11 is insufficient for the current Z3
source. Install a newer toolchain and set `CC=gcc-13 CXX=g++-13` when building
there. Release builds use GCC 13 on Ubuntu 22.04 and statically link its C++
standard library, so the compiler upgrade does not silently raise the required
system libstdc++ version. A vendored Z3 build can take several minutes. The CLI
build timeout is one hour. Dependencies are locked with `--locked`.

Without `--path`, `--from-source --rev FULL_COMMIT_SHA` fetches that revision;
`--from-source --version vX.Y.Z` fetches that tag. Without either, it uses the
checkout containing the script, or fetches `main` when no checkout is available.
Select a revision that contains this installer. Old revisions without the
`install` CLI cannot provide the managed lifecycle.

`CARGO_TARGET_DIR` reuses a build cache. `CARGO_BUILD_TARGET` selects the output
subdirectory and must produce an executable runnable on the installing host.
`--path` with `--rev` refuses dirty or mismatched trees. Omitting `--rev` deliberately
allows working-tree builds; `doctor` reports their build revision, including the
`-dirty` suffix. Do not set `SAIL_LSP_BUILD_REVISION` for local builds unless you
are intentionally overriding provenance for a controlled build.

`scripts/install-server.sh` is a compatibility entry point for this managed source
installer. Its arguments are installer flags; the old positional binary-directory
argument is rejected to avoid changing the meaning of an existing path.

## Editors

Pass one or several integrations when installing:

```sh
sh install.sh --editor vscode
sh install.sh --editor neovim,helix,emacs
sh install.sh --editor zed
# Local development, without publishing:
sh install.sh --path "$PWD" --editor vscode
```

Editor applications are installed separately. Repeating an integration updates
only its managed files, blocks or selected keys. An edited managed file or setting
causes installation to stop instead of overwriting it. Unrelated JSONC comments,
TOML settings and shell/Emacs configuration are preserved, including changes made
between upgrades. `--editor-root DIR` redirects integration files for testing or
nonstandard layouts; it is a configuration root, not an editor executable.

* **VS Code**: installs the platform VSIX with `code --install-extension --force`
  and registers the stable server path in `<integration registry>/vscode.json`.
  `--code-command PATH` selects another compatible CLI. `--vsix FILE` supplies an
  offline VSIX; its identity, version and bundled server hash must match the server.
  Source installation builds the VSIX locally and additionally needs Node.js 22+/npm.
  Server selection is explicit `sail-lsp.server.path`, then the managed registration,
  then the bundled server, then PATH. A stale or malformed managed registration
  produces a visible startup error. An old installed extension must be upgraded
  before it can read the new registration. The extension is installed after every
  other editor change has been written; an installation that then fails removes
  the extension again only when none was installed before it ran. Uninstall
  removes the registration but leaves the extension installed; remove it with
  the editor's extension UI/CLI.
* **Zed**: prepares pinned, MIT-licensed development-extension sources under
  `<root>/editors/zed` and sets `lsp.sail_lsp.binary.path` in Zed's settings. Run
  **zed: install dev extension** and select that directory. This is the official
  [development extension workflow](https://zed.dev/docs/extensions/developing-extensions);
  compilation needs Rust and may download grammar/build dependencies. This step
  is not silently replaced by writing to Zed's internal extension store. The
  pinned upstream revision and local changes are in `editors/zed/UPSTREAM`.
  Before uninstalling, remove the dev extension from Zed. Reinstall it after an
  upgrade if its extension code changes.
* **Neovim 0.11+**: writes `plugin/sail_lsp_managed.lua` using the built-in
  `vim.lsp.config` / `vim.lsp.enable` APIs. No nvim-lspconfig dependency. It handles
  Sail and manifest filetypes. The directory follows `NVIM_APPNAME` and the normal
  configuration root, including `%LOCALAPPDATA%` on Windows and `XDG_CONFIG_HOME`.
  Existing separately configured Sail clients may also start; disable those if
  choosing this managed integration.
* **Helix**: updates `languages.toml` using TOML parsing, adds a distinct
  `sail-lsp-managed` server and puts it first in the Sail language's server list.
  Existing servers and other languages are kept. Syntax grammars/themes are
  independent of the server and are not installed by this command.
* **Emacs 29+**: writes a small Eglot configuration and adds a marked loader block
  to the existing init file. Reuses `sail-mode` / `sail-project-mode` if already
  defined; otherwise provides basic derived modes. This supplies LSP integration,
  not a full Sail syntax-highlighting package.

For VS Code Remote SSH run installation on the remote host, in the remote
extension environment (the `code` CLI must target that environment). For Zed SSH
install the server remotely and set the remote server path according to Zed's
remote settings; do not assume a local binary path exists remotely. Remote editor
sessions are not covered by the automated native installation tests.

## Upgrade, rollback and uninstall

Run the bootstrap again to upgrade; repeat `--editor` when you also want updated
editor integration. The stable executable path is unchanged.

Retained versions are identified by their payload contents. Reinstalling the
same payload keeps the installation state unchanged. Rebuilding the same source
revision can produce different bytes (for example, MSVC timestamps and PDB
GUIDs); those builds are retained separately and can be selected for rollback.

```sh
sh install.sh                           # latest published release
sh install.sh --version vX.Y.Z           # pinned release
sh install.sh --list                     # retained versions and IDs
sh install.sh --rollback                 # switch to previous version
sh install.sh --rollback --version ID    # choose an unambiguous retained ID
sh install.sh --uninstall
```

These lifecycle operations work offline: the script runs a temporary copy of the
installed executable, which also avoids Windows locking itself during removal.
The equivalent `sail-lsp install --list` / `--rollback` commands are available;
on Windows use the PowerShell bootstrap for upgrades/uninstall and stop running
servers first. Always pass the same `--prefix` if using a custom installation.

Archives are checked before activation. Downloads, invalid archives, modified
owned files and configuration-write failures leave the previous active version
available. Activation and state/config writes roll back on errors. A per-prefix
lock prevents concurrent installers. Version directories are immutable and kept
for rollback; there is currently no automatic garbage collection. Rollback
switches the server executable, not external editor extensions. VSIX installation
is an external editor operation and cannot be rolled back as a filesystem
transaction; a failed integration may have installed a newer extension while
retaining the previous active server.

Uninstall removes only recorded, unchanged owned files and managed settings.
User-modified files and unrelated content remain, with a message for preserved
managed files. Empty directories may remain. The `.install.lock` file is retained
so concurrent processes never acquire different lock files. Remove the root
manually after uninstall if you no longer need any preserved content. An unrelated
binary at the stable executable path is never silently overwritten.

## Offline installation and mirrors

Transfer the archive and its `.sha256` sidecar, plus the installer script:

```sh
sh install.sh --archive ./sail-lsp-x86_64-unknown-linux-gnu.tar.gz --no-modify-path
# Or supply a separately obtained expected hash:
sh install.sh --archive ./server.tar.gz --sha256 EXPECTED_64_HEX_DIGEST
```

Windows uses the corresponding `.zip`. Include `--vsix FILE --editor vscode` for
an offline VS Code integration. `--release-base-url https://mirror.example/releases`
(or `SAIL_LSP_RELEASE_BASE_URL`) supports a mirror with the same
`download/TAG/ASSET` and `latest/download/ASSET` layout. An explicitly selected
`file://` mirror is supported for offline/testing; HTTP is rejected. A Windows
mirror is written `file:///C:/mirror` (a native path behind the scheme,
`file://C:\mirror`, also works); percent-escapes such as `%20` are decoded. The native
CLI's online downloader uses curl, including when invoked directly on Windows;
the PowerShell bootstrap downloads through Windows APIs instead.

## Inspecting a runtime

```sh
sail-lsp --version --json
sail-lsp doctor
sail-lsp doctor --json
```

Build metadata includes the source revision, target triple, embedded stdlib
revision, SMT linkage and static CRT flag. Doctor checks extraction of the
embedded library, a `SAIL_DIR` override, an actual SMT query, PATH copies, optional
upstream compiler version, and the VS Code registration. Required failures return
nonzero; an absent optional compiler is informational. It does not certify a
particular project's typechecking or remote editor integration.

## Platform matrix and evidence

`distribution/targets.json` is the build/packaging source of truth:

| Target | CI priority | Declared release runtime |
| --- | --- | --- |
| Linux x86_64 GNU | primary | glibc >= 2.35, libgcc_s |
| Windows x86_64 MSVC | primary | Windows 10+, static CRT |
| macOS ARM64 | primary | macOS 13.3+ deployment target |
| Linux ARM64 GNU | preview | glibc >= 2.39, libgcc_s |
| macOS x86_64 | preview | macOS 13.3+ deployment target |
| Windows ARM64 MSVC | preview | Windows 11+, static CRT |

The vendored Z3 build uses C++20 formatting APIs that require macOS 13.3.
Both release and locked-source CI builds use the deployment target declared in
the matrix above.

The matrix specifies build and deployment requirements. Consult the native
installation/release workflow artifacts for acceptance results on each runner;
those results do not cover every OS version above the declared minimum.
The local developer binary may require a newer glibc than the release floor;
packaging rejects that for a release and records the measured floor for explicitly
allowed dirty local packages. Alpine/musl, 32-bit systems and other targets do not
have prebuilt packages. Source builds on unlisted platforms are best effort.

## Release and CI maintenance

`.github/workflows/release-server.yaml` owns server releases. Tag pushes `v*` or
manual runs build native artifacts for all six targets, verify provenance and
SHA256, reuse the identical server bytes in the VSIX, and run lifecycle + real
LSP initialization/hover/shutdown tests. The primary editor targets also run the
VS Code Extension Host against that release binary. Version tags must match
both `crates/sail-lsp/Cargo.toml` and `editors/code/package.json`.

GitHub publishing waits for all native builds/tests. Marketplace publishing is
an independent opt-in job requiring `publish_marketplace` and `VSCE_PAT`; it is
not required for a server release. `release-vscode.yaml` is a manual compatibility
entry point that delegates to the same release workflow. A first standalone
release still needs a maintainer's tag/manual publishing action.

Main CI invokes `install.yaml`: native matrix gates, clean pinned source install,
Zed WASM build, and a network-disabled Ubuntu 22.04 runtime container without
Rust/Node/Python/system Z3. Integration tests use isolated configuration roots and
native executable version fixtures for upgrade/rollback; the actual release
binary separately runs doctor and a real LSP session. Linux ABI requirements are
measured during packaging. Windows runtime execution and Z3 queries check that
native dependencies are available on the runner; static CRT metadata is required.

For a local development package:

```sh
cargo build --release --locked -p sail-lsp --features vendored
python3 scripts/package-distribution.py pack --binary target/release/sail-lsp \
  --tag vX.Y.Z --output target/distribution/dist --allow-dirty
python3 scripts/package-distribution.py extract \
  target/distribution/dist/sail-lsp-x86_64-unknown-linux-gnu.tar.gz \
  target/distribution/payload
cd editors/code
npm ci
node scripts/package-server.mjs --distribution ../../target/distribution/payload
npm run package:vsix:linux-x64
```

`--allow-dirty` is for local testing only. Release workflows never pass it.


## Project and analysis commands

VS Code offers **Sail: Explain Analysis at Cursor**, **Sail: Preview Preprocessed
Source**, project navigation, include-context selection and project tasks in the
command palette. Preprocessing opens a read-only snapshot with source links.

The managed Neovim 0.11+ configuration installs these commands:

| Command | Action |
| --- | --- |
| `:SailActions` | All Sail project/analysis source actions |
| `:SailSelectProject` | Select the analysis project for this client session |
| `:SailContext` | Select an actual project/module/include visit, or automatic selection |
| `:SailProject` | Open the owning project manifest |
| `:SailExplain` | Explain types, rejected overloads, constraints and visibility |
| `:SailFormatInfo` | Explain formatting status, project style and effective options |
| `:SailPreprocess` | Preview the selected preprocessing visit |
| `:SailSource` / `gd` in a preview | Open a mapped source location |
| `:SailTasks` | Select a loaded-project check, test or export task |
| `:SailCancelTask` | Cancel the fallback process task and scoped compiler checks |

Project selection updates the running Neovim client's settings; persist
`settings["sail-lsp"].workspace.activeProject` in your configuration to keep it
between sessions. A per-document include selection is cleared when that source
buffer is closed. Generated preview buffers are read-only.

Zed uses its normal **editor: toggle code actions** menu: choose a **Sail:**
source action for context selection, navigation, analysis, preprocessing or
project tasks. Install the repository development extension to register the
Sail Analysis Preview language and source links. For persistent project
selection configure `lsp.sail_lsp.settings.workspace.activeProject`. These
entrypoints use standard LSP commands and document opening, avoiding an
unsupported custom-command API in the extension. Native Zed/SSH UI acceptance
is separate from the server protocol tests and the Neovim headless smoke test.

Project task configuration and the standard-client execution limits are
described in [project tasks](runnables.md). To repeat the native Neovim smoke
check, build the server and run:

```sh
SAIL_LSP_TEST_SERVER="$PWD/target/debug/sail-lsp" nvim --headless -u NONE -l scripts/neovim_smoke.lua
```
