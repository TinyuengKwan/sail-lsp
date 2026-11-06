/** One bounded SSR preview, guarded by server source texts and client versions.
 * VS Code serializes applyEdit with version IDs for these opened documents. */
import * as vscode from "vscode";
import { LanguageClient, WorkspaceEdit as ProtocolEdit } from "vscode-languageclient/node";
type Preview = {
    edit: ProtocolEdit;
    documents: { uri: string; version: number | null; text: string }[];
};
export function registerStructuralReplace(
    context: vscode.ExtensionContext,
    getClient: () => LanguageClient | undefined,
) {
    let pending:
        | {
              edit: vscode.WorkspaceEdit;
              sources: { document: vscode.TextDocument; version: number }[];
              panel: vscode.WebviewPanel;
              stale: boolean;
              dispose(): void;
          }
        | undefined;
    let sequence = 0;
    const contents = new Map<string, string>();
    context.subscriptions.push(
        vscode.workspace.registerTextDocumentContentProvider("sail-ssr", {
            provideTextDocumentContent: (uri) => contents.get(uri.toString()) ?? "Preview expired",
        }),
    );
    const cancel = () => {
        ++sequence;
        const current = pending;
        pending = undefined;
        current?.dispose();
        contents.clear();
    };
    const escape = (s: string) =>
        s.replace(
            /[&<>"']/g,
            (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
        );
    const link = (name: string, args: unknown[]) =>
        `command:${name}?${encodeURIComponent(JSON.stringify(args))}`;
    context.subscriptions.push(
        new vscode.Disposable(cancel),
        vscode.commands.registerCommand("sail-lsp.cancelStructuralReplace", cancel),
        vscode.commands.registerCommand("sail-lsp.applyStructuralReplace", async () => {
            const current = pending;
            if (!current) return false;
            if (
                current.stale ||
                current.sources.some(
                    ({ document, version }) => document.isClosed || document.version !== version,
                )
            ) {
                current.stale = true;
                void vscode.window.showWarningMessage(
                    "Sail SSR preview is stale. Run Structural Replace again.",
                );
                return false;
            }
            pending = undefined;
            current.dispose();
            contents.clear();
            const applied = await vscode.workspace.applyEdit(current.edit);
            if (!applied)
                void vscode.window.showWarningMessage(
                    "Sail SSR edit was rejected. Refresh the preview and try again.",
                );
            return applied;
        }),
        vscode.commands.registerCommand("sail-lsp.structuralReplace", async (rule?: string) => {
            const client = getClient(),
                editor = vscode.window.activeTextEditor;
            if (!client || editor?.document.languageId !== "sail") return 0;
            cancel();
            const request = sequence,
                document = editor.document,
                version = document.version;
            const params = {
                textDocument: { uri: document.uri.toString() },
                position: editor.selection.active,
                selections: editor.selection.isEmpty ? [] : [editor.selection],
            };
            const query =
                rule ??
                (await vscode.window.showInputBox({
                    title: "Sail: Structural Replace",
                    prompt: "search ==>> replacement; use $name for captures",
                    ignoreFocusOut: true,
                    validateInput: async (query) => {
                        if (!query.trim()) return undefined;
                        try {
                            await client.sendRequest("experimental/ssr", {
                                ...params,
                                query,
                                parseOnly: true,
                            });
                            return undefined;
                        } catch (error) {
                            return error instanceof Error ? error.message : String(error);
                        }
                    },
                }));
            if (
                !query?.trim() ||
                request !== sequence ||
                document.isClosed ||
                document.version !== version
            )
                return 0;
            try {
                const preview = await vscode.window.withProgress(
                    {
                        location: vscode.ProgressLocation.Notification,
                        title: "Preparing Sail structural replacement…",
                        cancellable: true,
                    },
                    async (_progress, token) => {
                        try {
                            const value = await client.sendRequest<Preview>(
                                "sail-lsp/ssrPreview",
                                { ...params, query, parseOnly: false },
                                token,
                            );
                            return token.isCancellationRequested ? undefined : value;
                        } catch (error) {
                            if (token.isCancellationRequested) return undefined;
                            throw error;
                        }
                    },
                );
                if (
                    !preview ||
                    request !== sequence ||
                    client !== getClient() ||
                    document.isClosed ||
                    document.version !== version
                )
                    return 0;
                const sources = await Promise.all(
                    preview.documents.map(async (base) => {
                        const doc = await vscode.workspace.openTextDocument(
                            vscode.Uri.parse(base.uri),
                        );
                        if (
                            doc.getText() !== base.text ||
                            (base.version !== null && doc.version !== base.version)
                        )
                            throw new Error(
                                "A source changed while SSR was running. Run Structural Replace again.",
                            );
                        return { document: doc, version: doc.version };
                    }),
                );
                if (request !== sequence) return 0;
                const edit = await client.protocol2CodeConverter.asWorkspaceEdit(preview.edit);
                if (!edit || edit.size === 0) {
                    void vscode.window.showInformationMessage("No matching Sail code.");
                    return 0;
                }
                if (
                    sources.some(
                        ({ document, version }) =>
                            document.isClosed || document.version !== version,
                    )
                )
                    throw new Error(
                        "A source changed while SSR was running. Run Structural Replace again.",
                    );
                const panel = vscode.window.createWebviewPanel(
                    "sail-ssr",
                    "Sail Structural Replace",
                    vscode.ViewColumn.Beside,
                    {
                        enableCommandUris: [
                            "sail-lsp.applyStructuralReplace",
                            "sail-lsp.cancelStructuralReplace",
                            "vscode.diff",
                        ],
                    },
                );
                const links: string[] = [];
                let count = 0;
                for (const base of preview.documents) {
                    const uri = vscode.Uri.parse(base.uri),
                        entries = edit.get(uri);
                    const index = sources.find(
                        (s) => s.document.uri.toString() === base.uri,
                    )!.document;
                    let after = base.text;
                    for (const e of [...entries].sort(
                        (a, b) => index.offsetAt(b.range.start) - index.offsetAt(a.range.start),
                    ))
                        after =
                            after.slice(0, index.offsetAt(e.range.start)) +
                            e.newText +
                            after.slice(index.offsetAt(e.range.end));
                    count += entries.length;
                    const beforeUri = vscode.Uri.from({
                        scheme: "sail-ssr",
                        path: `/${request}/before/${count}.sail`,
                    });
                    const afterUri = vscode.Uri.from({
                        scheme: "sail-ssr",
                        path: `/${request}/after/${count}.sail`,
                    });
                    contents.set(beforeUri.toString(), base.text);
                    contents.set(afterUri.toString(), after);
                    links.push(
                        `<li><a href="${escape(link("vscode.diff", [beforeUri, afterUri, uri.fsPath]))}">${escape(uri.fsPath)}</a> (${entries.length})</li>`,
                    );
                }
                panel.webview.html = `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline';"><h2>${count} replacements in ${sources.length} files</h2><p><a href="${link("sail-lsp.applyStructuralReplace", [])}">Apply all</a> · <a href="${link("sail-lsp.cancelStructuralReplace", [])}">Cancel</a></p><ul>${links.join("")}</ul>`;
                const changed = vscode.workspace.onDidChangeTextDocument((e) => {
                    if (
                        sources.some((s) => s.document === e.document) &&
                        pending?.panel === panel
                    ) {
                        pending.stale = true;
                        panel.webview.html =
                            "<p>The source has changed. Run Structural Replace again to refresh the preview.</p>";
                    }
                });
                const closed = vscode.workspace.onDidCloseTextDocument((doc) => {
                    if (sources.some((s) => s.document === doc) && pending?.panel === panel)
                        pending.stale = true;
                });
                const disposed = panel.onDidDispose(() => {
                    if (pending?.panel === panel) cancel();
                });
                pending = {
                    edit,
                    sources,
                    panel,
                    stale: false,
                    dispose: () => {
                        changed.dispose();
                        closed.dispose();
                        disposed.dispose();
                        panel.dispose();
                    },
                };
                return count;
            } catch (error) {
                if (request === sequence)
                    void vscode.window.showErrorMessage(
                        `Sail SSR: ${error instanceof Error ? error.message : String(error)}`,
                    );
                return 0;
            }
        }),
    );
    return { pending: () => pending?.edit.size ?? 0, stale: () => pending?.stale ?? false };
}
