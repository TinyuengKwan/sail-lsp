// Extension entry point for sail-lsp VS Code extension.
//
// Mirrors rust-analyzer's `editors/code/src/main.ts`:
//   - activate() creates LSP client connected to sail-lsp binary
//   - deactivate() stops the client gracefully
//   - Server binary resolved from config, PATH, or bundled

import * as vscode from "vscode";
import * as path from "path";
import * as os from "os";
import * as fs from "fs";
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
    TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel;
let activationErrorShown = false;

export async function activate(
    context: vscode.ExtensionContext,
): Promise<void> {
    outputChannel = vscode.window.createOutputChannel("Sail Language Server");
    context.subscriptions.push(outputChannel);

    const serverPath = getServerPath(context);
    if (!serverPath) {
        showMissingServerError();
        return;
    }

    outputChannel.appendLine(`[sail-lsp] using server: ${serverPath}`);

    // RA pattern: server options define how to start the LSP server.
    const serverOptions: ServerOptions = {
        command: serverPath,
        transport: TransportKind.stdio,
        options: {
            env: {
                ...process.env,
                ...getExtraEnv(),
            },
        },
    };

    // RA pattern: client options define document selector & capabilities.
    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: "file", language: "sail" }],
        outputChannel,
        traceOutputChannel: outputChannel,
        middleware: {},
        initializationOptions: getInitializationOptions(),
    };

    client = new LanguageClient(
        "sail-lsp",
        "Sail Language Server",
        serverOptions,
        clientOptions,
    );

    client.onDidChangeState((event) => {
        outputChannel.appendLine(
            `[sail-lsp] client state ${event.oldState} -> ${event.newState}`,
        );
    });

    // Register commands
    registerCommands(context);

    // Start the client (and server)
    try {
        await client.start();
    } catch (error) {
        client = undefined;
        const message = error instanceof Error ? error.message : String(error);
        outputChannel.appendLine(`[sail-lsp] failed to start: ${message}`);
        void vscode.window.showErrorMessage(
            `Failed to start sail-lsp: ${message}`,
        );
        return;
    }
    outputChannel.appendLine("[sail-lsp] server started");
}

export async function deactivate(): Promise<void> {
    if (client) {
        await client.stop();
        client = undefined;
    }
}

// ── Server binary resolution ─────────────────────────────────────
//
// Mirrors RA bootstrap.ts resolution order:
//   1. Explicit config: sail-lsp.server.path
//   2. PATH lookup
//   3. Bundled binary in extension

function getServerPath(context: vscode.ExtensionContext): string | undefined {
    const config = vscode.workspace.getConfiguration("sail-lsp");

    // 1. Explicit config override
    const configPath = config.get<string | null>("server.path");
    if (configPath) {
        // Support ${workspaceFolder} substitution
        const resolved = substituteVars(configPath);
        if (resolved) {
            return resolved;
        }
    }

    // 2. Bundled binary in extension
    const bundled = getBundledServerPath(context);
    if (bundled) {
        return bundled;
    }

    // 3. PATH lookup
    const ext = os.platform() === "win32" ? ".exe" : "";
    const which = findInPath(`sail-lsp${ext}`);
    if (which) {
        return which;
    }

    return undefined;
}

function findInPath(name: string): string | undefined {
    const pathDirs = (process.env["PATH"] ?? "").split(path.delimiter);
    for (const dir of pathDirs) {
        const candidate = path.join(dir, name);
        try {
            fs.accessSync(candidate, fs.constants.X_OK);
            return candidate;
        } catch {
            // continue
        }
    }
    return undefined;
}

function getBundledServerPath(
    context: vscode.ExtensionContext,
): string | undefined {
    const ext = os.platform() === "win32" ? ".exe" : "";
    const bundled = path.join(context.extensionPath, "server", `sail-lsp${ext}`);

    try {
        fs.accessSync(bundled, fs.constants.X_OK);
        return bundled;
    } catch {
        return undefined;
    }
}

function showMissingServerError(): void {
    if (activationErrorShown) {
        return;
    }
    activationErrorShown = true;
    void vscode.window.showErrorMessage(
        "sail-lsp binary not found. Set `sail-lsp.server.path`, install sail-lsp to $PATH, or use a VSIX with bundled binaries for your platform.",
    );
}

function substituteVars(p: string): string {
    const folders = vscode.workspace.workspaceFolders;
    if (folders && folders.length > 0) {
        p = p.replace("${workspaceFolder}", folders[0].uri.fsPath);
    }
    // Support ${env:VAR} substitution
    p = p.replace(/\$\{env:(\w+)\}/g, (_, name: string) => {
        return process.env[name] ?? "";
    });
    return p;
}

// ── Configuration ────────────────────────────────────────────────

function getExtraEnv(): Record<string, string> {
    const config = vscode.workspace.getConfiguration("sail-lsp");
    return config.get<Record<string, string>>("server.extraEnv") ?? {};
}

function getInitializationOptions(): unknown {
    // Pass all sail-lsp.* settings to the server as initializationOptions.
    // Mirrors RA pattern where config is sent as initialization options.
    const config = vscode.workspace.getConfiguration("sail-lsp");
    return {
        diagnostics: {
            enable: config.get<boolean>("diagnostics.enable", true),
            effectMismatch: config.get<boolean>(
                "diagnostics.effectMismatch",
                true,
            ),
            disabled: config.get<string[]>("diagnostics.disabled", []),
        },
        inlayHints: {
            enable: config.get<boolean>("inlayHints.enable", true),
            typeHints: config.get<boolean>("inlayHints.typeHints", true),
            parameterHints: config.get<boolean>(
                "inlayHints.parameterHints",
                true,
            ),
            effectHints: config.get<boolean>("inlayHints.effectHints", true),
            maxLength: config.get<number | null>("inlayHints.maxLength", 25),
        },
        completion: {
            enable: config.get<boolean>("completion.enable", true),
            addCallParenthesis: config.get<boolean>(
                "completion.addCallParenthesis",
                true,
            ),
            postfix: config.get<boolean>("completion.postfix", true),
            limit: config.get<number>("completion.limit", 200),
        },
        codeLens: {
            enable: config.get<boolean>("codeLens.enable", true),
        },
        hover: {
            docs: config.get<boolean>("hover.docs", true),
            keywords: config.get<boolean>("hover.keywords", true),
        },
        semanticTokens: {
            enable: config.get<boolean>("semanticTokens.enable", true),
        },
        z3: {
            timeoutMs: config.get<number>("z3.timeoutMs", 1000),
        },
        workspace: {
            maxFiles: config.get<number>("workspace.maxFiles", 10000),
            includePaths: config.get<string[]>(
                "workspace.includePaths",
                [],
            ),
        },
    };
}

// ── Commands ─────────────────────────────────────────────────────

function registerCommands(context: vscode.ExtensionContext): void {
    context.subscriptions.push(
        vscode.commands.registerCommand("sail-lsp.restartServer", async () => {
            if (client) {
                outputChannel.appendLine("[sail-lsp] restarting server...");
                await client.stop();
                await client.start();
                outputChannel.appendLine("[sail-lsp] server restarted");
            }
        }),

        vscode.commands.registerCommand("sail-lsp.reload", async () => {
            if (client) {
                await client.sendNotification("sail-lsp/reloadWorkspace");
                void vscode.window.showInformationMessage(
                    "Sail workspace reload requested",
                );
            }
        }),

        vscode.commands.registerCommand("sail-lsp.serverVersion", () => {
            if (client) {
                void vscode.window.showInformationMessage(
                    `sail-lsp server: ${getServerPath(context) ?? "unknown"}`,
                );
            } else {
                void vscode.window.showWarningMessage(
                    "sail-lsp server is not running",
                );
            }
        }),

        vscode.commands.registerCommand("sail-lsp.toggleInlayHints", () => {
            const config = vscode.workspace.getConfiguration("sail-lsp");
            const current = config.get<boolean>("inlayHints.enable", true);
            void config.update(
                "inlayHints.enable",
                !current,
                vscode.ConfigurationTarget.Global,
            );
        }),

        vscode.commands.registerCommand("sail-lsp.showSyntaxTree", async () => {
            if (!client) return;
            const editor = vscode.window.activeTextEditor;
            if (!editor) return;

            const content = await client.sendRequest<string>(
                "sail-lsp/viewSyntaxTree",
                {
                    textDocument: {
                        uri: editor.document.uri.toString(),
                    },
                },
            );
            await showVirtualTextDocument(content);
        }),

        vscode.commands.registerCommand("sail-lsp.viewHir", async () => {
            if (!client) return;
            const editor = vscode.window.activeTextEditor;
            if (!editor) return;

            const content = await client.sendRequest<string>("sail-lsp/viewHir", {
                textDocument: {
                    uri: editor.document.uri.toString(),
                },
                position: editor.selection.active,
            });
            await showVirtualTextDocument(content);
        }),

        vscode.commands.registerCommand("sail-lsp.viewItemTree", async () => {
            if (!client) return;
            const editor = vscode.window.activeTextEditor;
            if (!editor) return;

            const content = await client.sendRequest<string>(
                "sail-lsp/viewItemTree",
                {
                    textDocument: {
                        uri: editor.document.uri.toString(),
                    },
                },
            );
            await showVirtualTextDocument(content);
        }),
    );
}

async function showVirtualTextDocument(content: string): Promise<void> {
    const doc = await vscode.workspace.openTextDocument({
        content,
        language: "plaintext",
    });
    await vscode.window.showTextDocument(doc, vscode.ViewColumn.Beside, true);
}
