// The part of the Sail language configuration that is set at runtime, as
// rust-analyzer's `config.ts` does: the Enter rules for block comments.
// `language-configuration.json` holds the rest, and onEnterRules.ts the
// rules themselves, where a test can read them without an editor.
//
// Registered here rather than in the JSON because VS Code applies Enter
// rules read from the file only after a later registration; runtime rules
// reach every document opened after activation.

import * as vscode from "vscode";
import { BLOCK_COMMENT_ON_ENTER_RULES } from "./onEnterRules";

export function configureLanguage(context: vscode.ExtensionContext): void {
    context.subscriptions.push(
        vscode.languages.setLanguageConfiguration("sail", {
            onEnterRules: BLOCK_COMMENT_ON_ENTER_RULES.map((rule) => ({
                beforeText: rule.beforeText,
                afterText: rule.afterText,
                action: {
                    indentAction:
                        rule.indent === "indentOutdent"
                            ? vscode.IndentAction.IndentOutdent
                            : vscode.IndentAction.None,
                    appendText: rule.appendText,
                    removeText: rule.removeText,
                },
            })),
        }),
    );
}
