// The Enter rules for `/* */` comments, without the editor: what each one
// matches and appends. languageConfiguration.ts hands them to VS Code.
//
// `//` and `///` lines are the server's: `experimental/onEnter` (bound to
// Enter) and the `"\n"` on-type formatting trigger continue them, and see
// whether the comment goes on. A rule here as well would add a second
// prefix, so these stop at block comments.

export interface BlockCommentOnEnterRule {
    beforeText: RegExp;
    afterText?: RegExp;
    /** `vscode.IndentAction` by name. */
    indent: "none" | "indentOutdent";
    appendText?: string;
    removeText?: number;
}

export const BLOCK_COMMENT_ON_ENTER_RULES: readonly BlockCommentOnEnterRule[] = [
    {
        // Begins an auto-closed block comment: /* | */ or /** | */
        beforeText: /^\s*\/\*(\*|!)?(?!\/)([^*]|\*(?!\/))*$/,
        afterText: /^\s*\*\/$/,
        indent: "indentOutdent",
        appendText: " * ",
    },
    {
        // Begins a block comment: /* ...| or /** ...|
        beforeText: /^\s*\/\*(\*|!)?(?!\/)([^*]|\*(?!\/))*$/,
        indent: "none",
        appendText: " * ",
    },
    {
        // Continues one:  * ...|
        beforeText: /^(\t|( {2}))* \*( ([^*]|\*(?!\/))*)?$/,
        indent: "none",
        appendText: "* ",
    },
    {
        // Dedents after closing one:  */|
        beforeText: /^(\t|( {2}))* \*\/\s*$/,
        indent: "none",
        removeText: 1,
    },
];

/** The first rule that applies to Enter between `before` and `after`, as VS Code picks it. */
export function onEnterRule(before: string, after: string): BlockCommentOnEnterRule | undefined {
    return BLOCK_COMMENT_ON_ENTER_RULES.find(
        (rule) => rule.beforeText.test(before) && (!rule.afterText || rule.afterText.test(after)),
    );
}
