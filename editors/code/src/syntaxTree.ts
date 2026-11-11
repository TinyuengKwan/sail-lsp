// Reading the server's syntax tree dump — rust-analyzer's `syntax_tree.ts`
// parsed `NODE@start..end` the same way — kept free of `vscode` so the
// parsing and the offset arithmetic are testable on their own.

/** One line of `sail-lsp/viewSyntaxTree`: `  KIND@start..end` for a node,
 * `  KIND@start..end "text"` for a token; offsets are UTF-8 bytes. */
export function parseSyntaxTreeLine(
    line: string,
): { kind: string; start: number; end: number } | undefined {
    const match = /^\s*([A-Z_][A-Z0-9_]*)@(\d+)\.\.(\d+)(?:\s|$)/.exec(line);
    if (!match) return undefined;
    return { kind: match[1], start: Number(match[2]), end: Number(match[3]) };
}

/** The UTF-16 index in `text` of the character at UTF-8 byte `offset`; the
 * end of the text when the offset is past it. */
export function byteOffsetToCharOffset(text: string, offset: number): number {
    let bytes = 0;
    let index = 0;
    while (index < text.length) {
        if (bytes >= offset) return index;
        const code = text.codePointAt(index)!;
        bytes += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
        index += code >= 0x10000 ? 2 : 1;
    }
    return text.length;
}

export const VIEW_SCHEMES = {
    syntaxTree: "sail-lsp-syntax-tree",
    hir: "sail-lsp-hir",
    itemTree: "sail-lsp-item-tree",
} as const;
export type ViewKind = keyof typeof VIEW_SCHEMES;

/** A view is bound to one source document; the view URI carries it. */
export function viewUriParts(
    kind: ViewKind,
    source: string,
): { scheme: string; path: string; query: string } {
    const name = decodeURIComponent(source.slice(source.lastIndexOf("/") + 1)) || "source";
    const suffix = { syntaxTree: "syntax-tree", hir: "hir", itemTree: "item-tree" }[kind];
    return { scheme: VIEW_SCHEMES[kind], path: `/${name}.${suffix}.txt`, query: source };
}
