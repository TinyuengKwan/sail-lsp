// Live syntax tree, HIR and item tree views — rust-analyzer's
// `syntax_tree.ts` of old: a `TextDocumentContentProvider` per scheme whose
// documents follow the source they are bound to, re-requested as it is
// edited, and whose syntax-tree lines reveal the source range they name.

import * as vscode from "vscode";
import type { LanguageClient } from "vscode-languageclient/node";
import {
    VIEW_SCHEMES,
    ViewKind,
    byteOffsetToCharOffset,
    parseSyntaxTreeLine,
    viewUriParts,
} from "./syntaxTree";

type TextDocumentParams = { textDocument: { uri: string } };
const METHODS: Record<ViewKind, string> = {
    syntaxTree: "sail-lsp/viewSyntaxTree",
    hir: "sail-lsp/viewHir",
    itemTree: "sail-lsp/viewItemTree",
};

export function registerViews(
    context: vscode.ExtensionContext,
    getClient: () => LanguageClient | undefined,
): void {
    const changed = new vscode.EventEmitter<vscode.Uri>();
    /** The cursor each HIR view was last asked about. */
    const positions = new Map<string, vscode.Position>();
    const timers = new Map<string, NodeJS.Timeout>();
    const open = (): vscode.TextDocument[] =>
        vscode.workspace.textDocuments.filter((document) =>
            Object.values(VIEW_SCHEMES).includes(document.uri.scheme as never),
        );
    const refresh = (source: vscode.Uri, kinds: ViewKind[] = ["syntaxTree", "hir", "itemTree"]) => {
        for (const view of open()) {
            if (view.uri.query !== source.toString()) continue;
            const kind = (Object.keys(VIEW_SCHEMES) as ViewKind[]).find(
                (k) => VIEW_SCHEMES[k] === view.uri.scheme,
            );
            if (!kind || !kinds.includes(kind)) continue;
            const key = view.uri.toString();
            clearTimeout(timers.get(key));
            timers.set(
                key,
                setTimeout(() => {
                    timers.delete(key);
                    changed.fire(view.uri);
                }, 300),
            );
        }
    };
    const provider = (kind: ViewKind): vscode.TextDocumentContentProvider => ({
        onDidChange: changed.event,
        async provideTextDocumentContent(uri) {
            const client = getClient();
            if (!client) return "(the Sail server is not running)";
            const params: TextDocumentParams & { position?: vscode.Position } = {
                textDocument: { uri: uri.query },
            };
            if (kind === "hir") {
                params.position = positions.get(uri.toString()) ?? new vscode.Position(0, 0);
            }
            return client.sendRequest<string>(METHODS[kind], params);
        },
    });
    const show = async (kind: ViewKind): Promise<void> => {
        const editor = vscode.window.activeTextEditor;
        if (!getClient() || !editor) return;
        const uri = vscode.Uri.from(viewUriParts(kind, editor.document.uri.toString()));
        if (kind === "hir") positions.set(uri.toString(), editor.selection.active);
        const document = await vscode.workspace.openTextDocument(uri);
        await vscode.window.showTextDocument(document, vscode.ViewColumn.Beside, true);
    };
    /** Selects, in the source a syntax-tree view is bound to, the range the
     * view's line at `line` names. */
    const reveal = async (view: vscode.TextDocument, line: number): Promise<boolean> => {
        if (view.uri.scheme !== VIEW_SCHEMES.syntaxTree || line >= view.lineCount) return false;
        const element = parseSyntaxTreeLine(view.lineAt(line).text);
        if (!element) return false;
        const source = vscode.Uri.parse(view.uri.query);
        const document = await vscode.workspace.openTextDocument(source);
        const text = document.getText();
        const range = new vscode.Range(
            document.positionAt(byteOffsetToCharOffset(text, element.start)),
            document.positionAt(byteOffsetToCharOffset(text, element.end)),
        );
        const editor =
            vscode.window.visibleTextEditors.find(
                (candidate) => candidate.document.uri.toString() === source.toString(),
            ) ?? (await vscode.window.showTextDocument(document, { preserveFocus: true }));
        editor.selection = new vscode.Selection(range.start, range.end);
        editor.revealRange(range, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
        return true;
    };
    context.subscriptions.push(
        changed,
        ...(Object.keys(VIEW_SCHEMES) as ViewKind[]).map((kind) =>
            vscode.workspace.registerTextDocumentContentProvider(
                VIEW_SCHEMES[kind],
                provider(kind),
            ),
        ),
        vscode.workspace.onDidChangeTextDocument((event) => refresh(event.document.uri)),
        vscode.window.onDidChangeTextEditorSelection((event) => {
            const document = event.textEditor.document;
            if (document.uri.scheme === VIEW_SCHEMES.syntaxTree) {
                if (event.kind !== vscode.TextEditorSelectionChangeKind.Command) {
                    void reveal(document, event.selections[0].active.line);
                }
                return;
            }
            // A HIR view follows the cursor of the source it is bound to.
            for (const view of open()) {
                if (
                    view.uri.scheme !== VIEW_SCHEMES.hir ||
                    view.uri.query !== document.uri.toString()
                )
                    continue;
                positions.set(view.uri.toString(), event.selections[0].active);
                refresh(document.uri, ["hir"]);
            }
        }),
        vscode.workspace.onDidCloseTextDocument((document) => {
            positions.delete(document.uri.toString());
        }),
        { dispose: () => timers.forEach((timer) => clearTimeout(timer)) },
        vscode.commands.registerCommand("sail-lsp.showSyntaxTree", () => show("syntaxTree")),
        vscode.commands.registerCommand("sail-lsp.viewHir", () => show("hir")),
        vscode.commands.registerCommand("sail-lsp.viewItemTree", () => show("itemTree")),
        vscode.commands.registerCommand(
            "sail-lsp.syntaxTreeReveal",
            async (view?: string, line?: number) => {
                const document = view
                    ? await vscode.workspace.openTextDocument(vscode.Uri.parse(view))
                    : vscode.window.activeTextEditor?.document;
                if (!document) return false;
                return reveal(
                    document,
                    line ?? vscode.window.activeTextEditor?.selection.active.line ?? 0,
                );
            },
        ),
    );
}
