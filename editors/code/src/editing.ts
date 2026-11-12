// The server's editing requests — rust-analyzer's `matchingBrace`,
// `joinLines`, `onEnter` and `moveItem` extensions, which `commands.ts`
// wires up there. Each is offered only when the server advertised it in
// `experimental`; the fixture test `every_experimental_capability_has_a_handler`
// is what keeps the two lists equal on the server's side.

import * as vscode from "vscode";
import * as lc from "vscode-languageclient/node";
import { asTextEdits } from "./client";
import type { SnippetTextEdit } from "./snippets";

type Document = { textDocument: lc.TextDocumentIdentifier };

const MatchingBraceRequest = new lc.RequestType<
    Document & { positions: lc.Position[] },
    lc.Position[],
    void
>("experimental/matchingBrace");
const JoinLinesRequest = new lc.RequestType<Document & { ranges: lc.Range[] }, lc.TextEdit[], void>(
    "experimental/joinLines",
);
const OnEnterRequest = new lc.RequestType<
    lc.TextDocumentPositionParams,
    SnippetTextEdit[] | null,
    void
>("experimental/onEnter");
const MoveItemRequest = new lc.RequestType<
    Document & { range: lc.Range; direction: "Up" | "Down" },
    SnippetTextEdit[],
    void
>("experimental/moveItem");

/** Whether `document` changes within the next moment — the typed newline arriving. */
function documentChanged(document: vscode.TextDocument): Promise<boolean> {
    return new Promise((resolve) => {
        const subscription = vscode.workspace.onDidChangeTextDocument((event) => {
            if (event.document !== document) return;
            subscription.dispose();
            resolve(true);
        });
        setTimeout(() => {
            subscription.dispose();
            resolve(false);
        }, 1000);
    });
}

async function applySnippetEdits(
    client: lc.LanguageClient,
    editor: vscode.TextEditor,
    version: number,
    edits: SnippetTextEdit[],
): Promise<void> {
    if (editor.document.isClosed || editor.document.version !== version) return;
    const edit = new vscode.WorkspaceEdit();
    edit.set(editor.document.uri, asTextEdits(client, edits, editor.document));
    await vscode.workspace.applyEdit(edit);
}

export async function moveItem(
    client: lc.LanguageClient,
    editor: vscode.TextEditor,
    direction: "Up" | "Down",
): Promise<void> {
    const version = editor.document.version;
    const edits = await client.sendRequest(MoveItemRequest, {
        textDocument: client.code2ProtocolConverter.asTextDocumentIdentifier(editor.document),
        range: client.code2ProtocolConverter.asRange(editor.selection),
        direction,
    });
    if (edits.length) await applySnippetEdits(client, editor, version, edits);
}

export async function joinLines(
    client: lc.LanguageClient,
    editor: vscode.TextEditor,
): Promise<void> {
    const version = editor.document.version;
    const edits = await client.sendRequest(JoinLinesRequest, {
        textDocument: client.code2ProtocolConverter.asTextDocumentIdentifier(editor.document),
        ranges: editor.selections.map((selection) =>
            client.code2ProtocolConverter.asRange(selection),
        ),
    });
    const converted = await client.protocol2CodeConverter.asTextEdits(edits);
    if (editor.document.isClosed || editor.document.version !== version) return;
    await editor.edit((builder) => {
        for (const edit of converted) builder.replace(edit.range, edit.newText);
    });
}

export function registerEditingCommands(
    context: vscode.ExtensionContext,
    getClient: () => lc.LanguageClient | undefined,
): void {
    /** The running client and the Sail editor a command acts on, if both exist and the server offers the request. */
    const target = (
        capability: string,
    ): { client: lc.LanguageClient; editor: vscode.TextEditor } | undefined => {
        const client = getClient();
        const editor = vscode.window.activeTextEditor;
        if (!client || editor?.document.languageId !== "sail") return undefined;
        if (client.initializeResult?.capabilities.experimental?.[capability] !== true)
            return undefined;
        return { client, editor };
    };
    const move = async (direction: "Up" | "Down"): Promise<void> => {
        const found = target("moveItem");
        if (!found) return;
        await moveItem(found.client, found.editor, direction);
    };
    // Unlike rust-analyzer's, this server's `onEnter` is asked once the
    // newline is in the buffer: it looks at the fresh line and the one
    // above, as the `"\n"` on-type formatting trigger does. So the key
    // types first, and the answer completes the new line. The request is
    // skipped when `editor.formatOnType` is on, since the same edit is then
    // already on its way through `textDocument/onTypeFormatting`.
    const onEnter = async (): Promise<void> => {
        const found = target("onEnter");
        const selection =
            found?.editor.selections.length === 1 ? found.editor.selection : undefined;
        // `default:type` resolves before the change reaches this side, so
        // the fresh line is awaited, and found from where the cursor was.
        const typed = selection && found && documentChanged(found.editor.document);
        await vscode.commands.executeCommand("default:type", { text: "\n" });
        if (!found || !selection || !typed) return;
        const { client, editor } = found;
        if (vscode.workspace.getConfiguration("editor", editor.document).get("formatOnType"))
            return;
        if (!(await typed) || selection.start.line + 1 >= editor.document.lineCount) return;
        // The cursor sits after whatever indentation the editor added.
        const line = editor.document.lineAt(selection.start.line + 1);
        const position = new vscode.Position(
            line.lineNumber,
            line.firstNonWhitespaceCharacterIndex,
        );
        const version = editor.document.version;
        const edits = await client
            .sendRequest(OnEnterRequest, {
                textDocument: client.code2ProtocolConverter.asTextDocumentIdentifier(
                    editor.document,
                ),
                position: client.code2ProtocolConverter.asPosition(position),
            })
            .catch(() => null);
        // Typed-ahead text would be edited around a stale position.
        if (!edits?.length || editor.document.version !== version) return;
        await applySnippetEdits(client, editor, version, edits);
    };
    context.subscriptions.push(
        vscode.commands.registerCommand("sail-lsp.matchingBrace", async () => {
            const found = target("matchingBrace");
            if (!found) return;
            const { client, editor } = found;
            const positions = await client.sendRequest(MatchingBraceRequest, {
                textDocument: client.code2ProtocolConverter.asTextDocumentIdentifier(
                    editor.document,
                ),
                positions: editor.selections.map((selection) =>
                    client.code2ProtocolConverter.asPosition(selection.active),
                ),
            });
            // One answer per cursor; a cursor not on a bracket comes back
            // where it was. A selection keeps its anchor, so the match
            // extends it rather than collapsing it.
            editor.selections = editor.selections.map((selection, index) => {
                const position = positions[index];
                const active = position
                    ? client.protocol2CodeConverter.asPosition(position)
                    : selection.active;
                return new vscode.Selection(selection.isEmpty ? active : selection.anchor, active);
            });
            editor.revealRange(editor.selection);
        }),
        vscode.commands.registerCommand("sail-lsp.joinLines", async () => {
            const found = target("joinLines");
            if (!found) return;
            await joinLines(found.client, found.editor);
        }),
        vscode.commands.registerCommand("sail-lsp.onEnter", onEnter),
        vscode.commands.registerCommand("sail-lsp.moveItemUp", () => move("Up")),
        vscode.commands.registerCommand("sail-lsp.moveItemDown", () => move("Down")),
    );
}
