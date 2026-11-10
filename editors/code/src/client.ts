// What the client tells the server it can take beyond the protocol, and
// the conversions those extensions need. rust-analyzer's `client.ts`.

import * as vscode from "vscode";
import * as lc from "vscode-languageclient/node";
import {
    SnippetTextEdit,
    SnippetWorkspaceEdit,
    hasSnippetEdits,
    isSnippetEdit,
    unindentSnippet,
} from "./snippets";
import { CommandLink, HoverWithActions, renderHoverActions } from "./hoverActions";

/**
 * The `experimental` client capabilities. The server reads each key in
 * `ClientCapabilities` (`crates/sail-lsp/src/lsp/capabilities.rs`); a key
 * missing here is a feature the server quietly withholds.
 */
export const EXPERIMENTAL_CAPABILITIES = {
    snippetTextEdit: true,
    serverStatusNotification: true,
    hoverActions: true,
    // The client-side commands a code lens may run; each is registered in
    // main.ts with the protocol arguments the server sends.
    commands: { commands: ["sail-lsp.showReferences"] },
};

export class ExperimentalFeatures implements lc.StaticFeature {
    fillClientCapabilities(capabilities: lc.ClientCapabilities): void {
        capabilities.experimental = { ...EXPERIMENTAL_CAPABILITIES, ...capabilities.experimental };
    }
    getState(): lc.FeatureState {
        return { kind: "static" };
    }
    initialize(): void {}
    clear(): void {}
}

/**
 * Snippet edits as the editor applies them: interactively in the active
 * editor, as text anywhere else. The document is needed for the
 * indentation VS Code will add back.
 */
export function asTextEdits(
    client: lc.LanguageClient,
    edits: SnippetTextEdit[],
    document: vscode.TextDocument | undefined,
): (vscode.TextEdit | vscode.SnippetTextEdit)[] {
    return edits.map((edit) => {
        const range = client.protocol2CodeConverter.asRange(edit.range);
        if (!document || !isSnippetEdit(edit)) return new vscode.TextEdit(range, edit.newText);
        const line = document.lineAt(range.start.line);
        const indent = line.text.slice(0, line.firstNonWhitespaceCharacterIndex);
        return new vscode.SnippetTextEdit(
            range,
            new vscode.SnippetString(unindentSnippet(edit.newText, indent)),
        );
    });
}

async function asWorkspaceEdit(
    client: lc.LanguageClient,
    edit: SnippetWorkspaceEdit,
): Promise<vscode.WorkspaceEdit> {
    // The client's converter reads a `TextEdit` and would insert `$0` as
    // text, so only the resource operations go through it.
    const result = await client.protocol2CodeConverter.asWorkspaceEdit({
        documentChanges: edit.documentChanges,
        changeAnnotations: edit.changeAnnotations,
    });
    for (const [uri, edits] of Object.entries(edit.changes ?? {})) {
        const target = client.protocol2CodeConverter.asUri(uri);
        const document = edits.some(isSnippetEdit)
            ? await vscode.workspace.openTextDocument(target)
            : undefined;
        result.set(target, asTextEdits(client, edits, document));
    }
    return result;
}

/**
 * Code actions whose edits keep their snippets. The server answers both
 * requests with `lsp_ext::CodeAction`, whose edit is a
 * `SnippetWorkspaceEdit`; everything else about an action is converted as
 * the client would, so `data` still rides along to `codeAction/resolve`.
 */
export function codeActionMiddleware(
    getClient: () => lc.LanguageClient | undefined,
): Pick<lc.Middleware, "provideCodeActions" | "resolveCodeAction"> {
    const convert = async (
        client: lc.LanguageClient,
        item: lc.Command | lc.CodeAction,
        token: vscode.CancellationToken,
    ): Promise<vscode.Command | vscode.CodeAction> => {
        if (lc.Command.is(item)) return client.protocol2CodeConverter.asCommand(item);
        const action = await client.protocol2CodeConverter.asCodeAction(item, token);
        const edit = item.edit as SnippetWorkspaceEdit | undefined;
        if (edit && hasSnippetEdits(edit)) action.edit = await asWorkspaceEdit(client, edit);
        return action;
    };
    return {
        async provideCodeActions(document, range, context, token, next) {
            const client = getClient();
            if (!client) return next(document, range, context, token);
            const params: lc.CodeActionParams = {
                textDocument: client.code2ProtocolConverter.asTextDocumentIdentifier(document),
                range: client.code2ProtocolConverter.asRange(range),
                context: await client.code2ProtocolConverter.asCodeActionContext(context, token),
            };
            return client.sendRequest(lc.CodeActionRequest.type, params, token).then(
                (items) => items && Promise.all(items.map((item) => convert(client, item, token))),
                (error) =>
                    client.handleFailedRequest(lc.CodeActionRequest.type, token, error, null),
            );
        },
        async resolveCodeAction(item, token, next) {
            const client = getClient();
            if (!client) return next(item, token);
            return client
                .sendRequest(
                    lc.CodeActionResolveRequest.type,
                    client.code2ProtocolConverter.asCodeActionSync(item),
                    token,
                )
                .then(
                    (resolved) => convert(client, resolved, token) as Promise<vscode.CodeAction>,
                    (error) =>
                        client.handleFailedRequest(
                            lc.CodeActionResolveRequest.type,
                            token,
                            error,
                            item,
                        ),
                );
        },
    };
}

/** The commands behind the current hover's action links, by index. */
let hoverCommands: CommandLink[] = [];
const HOVER_COMMAND_PROXY = "sail-lsp.hoverRefCommandProxy";

/**
 * Hovers with the server's action links rendered under them. The client's
 * converter drops the `actions` the server adds (`hover_actions::Hover`),
 * so the request is sent here and the links appended as trusted Markdown.
 */
export function hoverMiddleware(
    getClient: () => lc.LanguageClient | undefined,
): Pick<lc.Middleware, "provideHover"> {
    return {
        async provideHover(document, position, token, next) {
            const client = getClient();
            if (!client) return next(document, position, token);
            const params: lc.HoverParams = {
                textDocument: client.code2ProtocolConverter.asTextDocumentIdentifier(document),
                position: client.code2ProtocolConverter.asPosition(position),
            };
            return client
                .sendRequest(lc.HoverRequest.type, params, token)
                .then(async (result: HoverWithActions | null) => {
                    if (!result) return null;
                    const hover = await client.protocol2CodeConverter.asHover(result);
                    if (result.actions?.length) {
                        const { markdown, commands } = renderHoverActions(
                            result.actions,
                            HOVER_COMMAND_PROXY,
                        );
                        hoverCommands = commands;
                        const links = new vscode.MarkdownString(markdown);
                        links.isTrusted = true;
                        hover.contents.push(links);
                    }
                    return hover;
                })
                .catch((error) =>
                    client.handleFailedRequest(lc.HoverRequest.type, token, error, null),
                );
        },
    };
}

/**
 * The command a hover action link runs — rust-analyzer's
 * `hoverRefCommandProxy`: a `showReferences` carries every location, far
 * too long for a command URI, so the link names an index instead.
 */
export function registerHoverCommandProxy(context: vscode.ExtensionContext): void {
    context.subscriptions.push(
        vscode.commands.registerCommand(HOVER_COMMAND_PROXY, async (index: number) => {
            const link = hoverCommands[index];
            if (link) await vscode.commands.executeCommand(link.command, ...(link.arguments ?? []));
        }),
    );
}
