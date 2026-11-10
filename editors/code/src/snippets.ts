// `SnippetTextEdit`, rust-analyzer's extension: a text edit whose text may
// carry `$0` or `${1:x}`. The protocol has no snippet in a workspace edit,
// so the server marks one with `insertTextFormat: 2` once the client has
// declared `experimental.snippetTextEdit` (client.ts) and strips the
// placeholders for a client that has not. This is the part that needs no
// editor; client.ts turns the edits into `vscode.SnippetTextEdit`s.

import type * as lc from "vscode-languageclient/node";

/** The wire shape of `lsp_ext::SnippetTextEdit`. */
export interface SnippetTextEdit {
    range: lc.Range;
    newText: string;
    /** 1 is plain text, 2 a snippet; absent means plain. */
    insertTextFormat?: number;
}

/** The wire shape of `lsp_ext::SnippetWorkspaceEdit`: only `changes` carry snippets. */
export interface SnippetWorkspaceEdit {
    changes?: Record<string, SnippetTextEdit[]>;
    documentChanges?: lc.WorkspaceEdit["documentChanges"];
    changeAnnotations?: lc.WorkspaceEdit["changeAnnotations"];
}

/**
 * Whether an edit is to be expanded: marked as a snippet and holding a
 * tabstop. A marked edit with nothing to expand is applied as text, as a
 * `vscode.SnippetTextEdit` would otherwise park the cursor at its end.
 */
export function isSnippetEdit(edit: SnippetTextEdit): boolean {
    return edit.insertTextFormat === 2 && /\$\d+|\$\{\d+:[^}]*\}/.test(edit.newText);
}

/** Whether a workspace edit carries anything to expand. */
export function hasSnippetEdits(edit: SnippetWorkspaceEdit): boolean {
    return Object.values(edit.changes ?? {}).some((edits) => edits.some(isSnippetEdit));
}

/**
 * Drop the indentation the server put on a multi-line snippet's
 * continuation lines. VS Code re-indents every line after the first to the
 * line it inserts at, so leaving the server's in doubles it up;
 * rust-analyzer's `removeLeadingWhitespace`.
 */
export function unindentSnippet(text: string, indent: string): string {
    const newline = text.indexOf("\n");
    if (newline === -1 || indent === "") return text;
    const rest = text
        .slice(newline + 1)
        .split("\n")
        .map((line) => (line.startsWith(indent) ? line.slice(indent.length) : line))
        .join("\n");
    return text.slice(0, newline + 1) + rest;
}
