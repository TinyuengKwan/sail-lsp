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
import { ProjectGraphEntry, GraphNode, graphChildren, isCycle } from "./projectGraph";
import { AnalysisContext, formatAnalysisContext } from "./analysisContext";
import {
    LanguageClient,
    Location as ProtocolLocation,
    LanguageClientOptions,
    NotificationType,
    NotificationType0,
    RequestType,
    RequestType0,
    ServerOptions,
    TransportKind,
} from "vscode-languageclient/node";

type TextDocumentParams = { textDocument: { uri: string } };
type FlycheckScope = { textDocument?: { uri: string } };

const ProjectGraphRequest = new RequestType0<ProjectGraphEntry[], void>("sail-lsp/projectGraph");
const StructuralSearchRequest = new RequestType<
    TextDocumentParams & { query: string; position: vscode.Position; selections: vscode.Range[] },
    ProtocolLocation[],
    void
>("sail-lsp/ssrSearch");
const AnalyzerStatusRequest = new RequestType0<string, void>("sail-lsp/analyzerStatus");
const ExplainAnalysisRequest = new RequestType<
    TextDocumentParams & { position: vscode.Position },
    AnalysisContext | null,
    void
>("sail-lsp/explainAnalysis");
const OpenProjectManifestRequest = new RequestType<TextDocumentParams, unknown, void>(
    "sail-lsp/openProjectManifest",
);
const ParentModuleRequest = new RequestType<TextDocumentParams, unknown[], void>(
    "sail-lsp/parentModule",
);
const ViewSyntaxTreeRequest = new RequestType<TextDocumentParams, string, void>(
    "sail-lsp/viewSyntaxTree",
);
const ViewHirRequest = new RequestType<
    TextDocumentParams & { position: vscode.Position },
    string,
    void
>("sail-lsp/viewHir");
const ViewItemTreeRequest = new RequestType<TextDocumentParams, string, void>(
    "sail-lsp/viewItemTree",
);
const RecheckNotification = new NotificationType0("sail-lsp/recheck");
const ReloadWorkspaceNotification = new NotificationType0("sail-lsp/reloadWorkspace");
const RunFlycheckNotification = new NotificationType<FlycheckScope>("sail-lsp/runFlycheck");
const CancelFlycheckNotification = new NotificationType<FlycheckScope>("sail-lsp/cancelFlycheck");
const ClearFlycheckNotification = new NotificationType<FlycheckScope>("sail-lsp/clearFlycheck");

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel;
let statusBar: vscode.StatusBarItem;
let activationErrorShown = false;
let latestStatus: ServerStatus | undefined;
let projectGraph: ProjectGraphEntry[] = [];

export interface ExtensionApi {
    test: {
        status(): ServerStatus | undefined;
        projectGraph(): readonly ProjectGraphEntry[];
    };
}

export async function activate(context: vscode.ExtensionContext): Promise<ExtensionApi | void> {
    outputChannel = vscode.window.createOutputChannel("Sail Language Server");
    context.subscriptions.push(outputChannel);

    // RA's status bar item: health and whether the server is still working,
    // the last message on hover, the full status on click.
    statusBar = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left);
    statusBar.command = "sail-lsp.analyzerStatus";
    statusBar.text = "$(sync~spin) Sail";
    statusBar.tooltip = "Sail: starting";
    statusBar.show();
    context.subscriptions.push(statusBar);

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
        // Manifests too: the server answers definition, completion and
        // diagnostics in them, and never analyses them as Sail.
        documentSelector: [
            { scheme: "file", language: "sail" },
            { scheme: "file", language: "sail-project" },
        ],
        outputChannel,
        traceOutputChannel: outputChannel,
        middleware: {},
        initializationOptions: getInitializationOptions(),
        synchronize: { configurationSection: "sail-lsp" },
    };

    client = new LanguageClient("sail-lsp", "Sail Language Server", serverOptions, clientOptions);
    client.onNotification("sail-lsp/serverStatus", (status: ServerStatus) => showStatus(status));

    client.onDidChangeState((event) => {
        outputChannel.appendLine(`[sail-lsp] client state ${event.oldState} -> ${event.newState}`);
    });

    // Register commands
    registerCommands(context);

    // Start the client (and server)
    try {
        await client.start();
    } catch (error) {
        client = undefined;
        showStatus({ health: "error", quiescent: true, message: "The server failed to start." });
        const message = error instanceof Error ? error.message : String(error);
        outputChannel.appendLine(`[sail-lsp] failed to start: ${message}`);
        void vscode.window.showErrorMessage(`Failed to start sail-lsp: ${message}`);
        return;
    }
    outputChannel.appendLine("[sail-lsp] server started");
    return {
        test: {
            status: () => latestStatus,
            projectGraph: () => projectGraph,
        },
    };
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

function getBundledServerPath(context: vscode.ExtensionContext): string | undefined {
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
    // The whole `sail-lsp` section, as rust-analyzer sends its own: a hand
    // copied subset drifted from package.json (it lacked generatedFiles and
    // flycheck and still sent removed settings).
    return JSON.parse(JSON.stringify(vscode.workspace.getConfiguration("sail-lsp")));
}

// ── Status ───────────────────────────────────────────────────────

interface ServerStatus {
    health: "ok" | "warning" | "error";
    quiescent: boolean;
    message?: string;
}

function showStatus(status: ServerStatus): void {
    latestStatus = status;
    const icon = !status.quiescent
        ? "$(sync~spin)"
        : status.health === "error"
          ? "$(error)"
          : status.health === "warning"
            ? "$(warning)"
            : "$(check)";
    statusBar.text = `${icon} Sail`;
    statusBar.backgroundColor =
        status.health === "error"
            ? new vscode.ThemeColor("statusBarItem.errorBackground")
            : status.health === "warning"
              ? new vscode.ThemeColor("statusBarItem.warningBackground")
              : undefined;
    const tooltip = new vscode.MarkdownString("", true);
    tooltip.appendText(status.message ?? (status.quiescent ? "Sail: ready" : "Sail: loading"));
    tooltip.appendMarkdown(
        "\n\n[Show status](command:sail-lsp.analyzerStatus) · [Recheck](command:sail-lsp.recheck) · [Cancel](command:sail-lsp.cancelFlycheck)",
    );
    tooltip.isTrusted = {
        enabledCommands: ["sail-lsp.analyzerStatus", "sail-lsp.recheck", "sail-lsp.cancelFlycheck"],
    };
    statusBar.tooltip = tooltip;
}

// ── Commands ─────────────────────────────────────────────────────

function registerCommands(context: vscode.ExtensionContext): void {
    const changed = new vscode.EventEmitter<GraphNode | undefined>();
    context.subscriptions.push(
        changed,
        vscode.window.createTreeView("sail-lsp.projectGraph", {
            treeDataProvider: {
                onDidChangeTreeData: changed.event,
                getChildren: (node) =>
                    node
                        ? graphChildren(node)
                        : projectGraph.map((project) => ({ kind: "project", project })),
                getTreeItem: (node) => {
                    const children = graphChildren(node);
                    const item = new vscode.TreeItem(
                        "",
                        children.length
                            ? vscode.TreeItemCollapsibleState.Collapsed
                            : vscode.TreeItemCollapsibleState.None,
                    );
                    if (node.kind === "file") {
                        item.resourceUri = vscode.Uri.parse(node.uri);
                        item.label = path.basename(item.resourceUri.fsPath);
                        item.command = {
                            command: "vscode.open",
                            title: "Open file",
                            arguments: [item.resourceUri],
                        };
                    } else if (node.kind === "project") {
                        item.label = node.project.path;
                        item.description = node.project.preferred ? "preferred context" : undefined;
                        item.tooltip = `Roots: ${node.project.roots.join(", ") || "all"}\nVariables: ${JSON.stringify(node.project.variables)}`;
                        item.command = {
                            command: "vscode.open",
                            title: "Open manifest",
                            arguments: [vscode.Uri.parse(node.project.manifest)],
                        };
                    } else {
                        const module = node.project.modules.find(
                            (module) => module.name === node.name,
                        );
                        item.label = node.name;
                        item.description = isCycle(node)
                            ? "cycle"
                            : !module
                              ? "unavailable"
                              : !module.active
                                ? "inactive"
                                : node.ancestors.length
                                  ? "required module"
                                  : undefined;
                        item.tooltip = `Requires: ${module?.requires.join(", ") || "none"}`;
                        item.iconPath = new vscode.ThemeIcon(
                            isCycle(node) ? "warning" : "symbol-module",
                        );
                    }
                    return item;
                },
            },
        }),
    );
    context.subscriptions.push(
        vscode.commands.registerCommand("sail-lsp.restartServer", async () => {
            if (client) {
                outputChannel.appendLine("[sail-lsp] restarting server...");
                await client.stop();
                await client.start();
                outputChannel.appendLine("[sail-lsp] server restarted");
            }
        }),

        vscode.commands.registerCommand("sail-lsp.selectProject", async () => {
            const projects = await client?.sendRequest(ProjectGraphRequest);
            if (!projects?.length) {
                void vscode.window.showInformationMessage("No Sail project is loaded.");
                return;
            }
            const choice = await vscode.window.showQuickPick(
                projects.map((project) => ({
                    label: `${project.preferred ? "$(check) " : ""}${vscode.Uri.parse(project.manifest).fsPath}`,
                    detail: `roots: ${project.roots.join(", ") || "all"}; variables: ${JSON.stringify(project.variables)}`,
                    project,
                })),
                { placeHolder: "Choose the analysis context for files shared by projects" },
            );
            if (choice)
                await vscode.workspace
                    .getConfiguration("sail-lsp")
                    .update(
                        "workspace.activeProject",
                        choice.project.path,
                        vscode.ConfigurationTarget.Workspace,
                    );
        }),
        vscode.commands.registerCommand("sail-lsp.showProjectGraph", async () => {
            // Opening the view is immediate. Populate it when the server
            // answers; a busy analysis worker must not leave the command
            // palette invocation itself pending.
            void vscode.commands.executeCommand("sail-lsp.projectGraph.focus");
            void client
                ?.sendRequest(ProjectGraphRequest)
                .then((projects) => {
                    projectGraph = projects ?? [];
                    changed.fire(undefined);
                })
                .catch((error: unknown) => {
                    outputChannel.appendLine(`[sail-lsp] project graph failed: ${String(error)}`);
                });
        }),
        vscode.commands.registerCommand("sail-lsp.recheck", async () => {
            await client?.sendNotification(RecheckNotification);
        }),
        vscode.commands.registerCommand("sail-lsp.recheckProject", async () => {
            await client?.sendNotification(RunFlycheckNotification, flycheckScope());
        }),
        vscode.commands.registerCommand("sail-lsp.cancelFlycheck", async () => {
            await client?.sendNotification(CancelFlycheckNotification, flycheckScope());
        }),
        vscode.commands.registerCommand("sail-lsp.clearFlycheck", async () => {
            await client?.sendNotification(ClearFlycheckNotification, flycheckScope());
        }),
        vscode.commands.registerCommand("sail-lsp.reload", async () => {
            if (client) {
                await client.sendNotification(ReloadWorkspaceNotification);
                void vscode.window.showInformationMessage("Sail workspace reload requested");
            }
        }),

        vscode.commands.registerCommand("sail-lsp.serverVersion", () => {
            if (client) {
                void vscode.window.showInformationMessage(
                    `sail-lsp server: ${getServerPath(context) ?? "unknown"}`,
                );
            } else {
                void vscode.window.showWarningMessage("sail-lsp server is not running");
            }
        }),

        vscode.commands.registerCommand("sail-lsp.toggleInlayHints", () => {
            const config = vscode.workspace.getConfiguration("sail-lsp");
            const current = config.get<boolean>("inlayHints.enable", true);
            void config.update("inlayHints.enable", !current, vscode.ConfigurationTarget.Global);
        }),

        vscode.commands.registerCommand("sail-lsp.analyzerStatus", async () => {
            if (!client) return;
            const content = await client.sendRequest(AnalyzerStatusRequest);
            await showVirtualTextDocument(content);
        }),

        vscode.commands.registerCommand("sail-lsp.explainAnalysis", async () => {
            const editor = vscode.window.activeTextEditor;
            if (!client || !editor || editor.document.languageId !== "sail") return;
            const result = await client.sendRequest(ExplainAnalysisRequest, {
                textDocument: { uri: editor.document.uri.toString() },
                position: client.code2ProtocolConverter.asPosition(editor.selection.active),
            });
            if (result) await showVirtualTextDocument(formatAnalysisContext(result));
            else
                void vscode.window.showInformationMessage(
                    "Analysis is not available for this file yet.",
                );
        }),

        vscode.commands.registerCommand("sail-lsp.openProjectManifest", async () => {
            const editor = vscode.window.activeTextEditor;
            if (!client || !editor) return;
            const location = await client.sendRequest(OpenProjectManifestRequest, {
                textDocument: { uri: editor.document.uri.toString() },
            });
            if (!location) {
                void vscode.window.showInformationMessage(
                    "This file is not part of a loaded Sail project.",
                );
                return;
            }
            await showLocation(client.protocol2CodeConverter.asLocation(location as never));
        }),

        vscode.commands.registerCommand("sail-lsp.structuralSearch", async (pattern?: string) => {
            const editor = vscode.window.activeTextEditor;
            const currentClient = client;
            if (!currentClient || !editor || editor.document.languageId !== "sail") return [];
            const document = editor.document;
            const version = document.version;
            const position = editor.selection.active;
            const query =
                pattern ??
                (await vscode.window.showInputBox({
                    title: "Sail: Structural Search",
                    prompt: "Expression pattern; use $name for captures (for example, combine($a, $b))",
                    value: document.getText(editor.selection),
                    ignoreFocusOut: true,
                }));
            if (!query?.trim() || document.version !== version || document.isClosed) return [];
            return vscode.window.withProgress(
                {
                    location: vscode.ProgressLocation.Notification,
                    title: "Searching Sail expressions…",
                    cancellable: true,
                },
                async (_progress, token) => {
                    try {
                        const matches = await currentClient.sendRequest(
                            StructuralSearchRequest,
                            {
                                query,
                                textDocument: { uri: document.uri.toString() },
                                position,
                                selections: [],
                            },
                            token,
                        );
                        if (
                            token.isCancellationRequested ||
                            document.version !== version ||
                            document.isClosed
                        )
                            return [];
                        const locations = matches.map((match) =>
                            currentClient.protocol2CodeConverter.asLocation(match),
                        );
                        await vscode.commands.executeCommand(
                            "editor.action.showReferences",
                            document.uri,
                            position,
                            locations,
                        );
                        return locations;
                    } catch (error) {
                        if (
                            !token.isCancellationRequested &&
                            document.version === version &&
                            !document.isClosed
                        ) {
                            const message = error instanceof Error ? error.message : String(error);
                            outputChannel.appendLine(
                                `[sail-lsp] structural search failed: ${message}`,
                            );
                            void vscode.window.showErrorMessage(
                                `Sail structural search failed: ${message}`,
                            );
                        }
                        return [];
                    }
                },
            );
        }),

        vscode.commands.registerCommand("sail-lsp.parentModule", async () => {
            const editor = vscode.window.activeTextEditor;
            if (!client || !editor) return;
            const locations = await client.sendRequest(ParentModuleRequest, {
                textDocument: { uri: editor.document.uri.toString() },
            });
            const converted = locations.map((l) =>
                client!.protocol2CodeConverter.asLocation(l as never),
            );
            if (converted.length === 0) {
                void vscode.window.showInformationMessage(
                    "Nothing in the project lists or includes this file.",
                );
            } else if (converted.length === 1) {
                await showLocation(converted[0]);
            } else {
                await vscode.commands.executeCommand(
                    "editor.action.goToLocations",
                    editor.document.uri,
                    editor.selection.active,
                    converted,
                    "peek",
                    "Nothing in the project lists or includes this file.",
                );
            }
        }),

        vscode.commands.registerCommand("sail-lsp.showSyntaxTree", async () => {
            if (!client) return;
            const editor = vscode.window.activeTextEditor;
            if (!editor) return;

            const content = await client.sendRequest(ViewSyntaxTreeRequest, {
                textDocument: {
                    uri: editor.document.uri.toString(),
                },
            });
            await showVirtualTextDocument(content);
        }),

        vscode.commands.registerCommand("sail-lsp.viewHir", async () => {
            if (!client) return;
            const editor = vscode.window.activeTextEditor;
            if (!editor) return;

            const content = await client.sendRequest(ViewHirRequest, {
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

            const content = await client.sendRequest(ViewItemTreeRequest, {
                textDocument: {
                    uri: editor.document.uri.toString(),
                },
            });
            await showVirtualTextDocument(content);
        }),
    );
}

function flycheckScope(): { textDocument?: { uri: string } } {
    const document = vscode.window.activeTextEditor?.document;
    return document && (document.languageId === "sail" || document.languageId === "sail-project")
        ? { textDocument: { uri: document.uri.toString() } }
        : {};
}

async function showLocation(location: vscode.Location): Promise<void> {
    const editor = await vscode.window.showTextDocument(location.uri);
    editor.selection = new vscode.Selection(location.range.start, location.range.end);
    editor.revealRange(location.range, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
}

async function showVirtualTextDocument(content: string): Promise<void> {
    const doc = await vscode.workspace.openTextDocument({
        content,
        language: "plaintext",
    });
    await vscode.window.showTextDocument(doc, vscode.ViewColumn.Beside, true);
}
