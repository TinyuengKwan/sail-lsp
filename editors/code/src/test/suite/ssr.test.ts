import * as assert from "node:assert/strict";
import * as vscode from "vscode";
import { replacementPreviews } from "../../ssr";

suite("SSR source identities", () => {
    test("equivalent protocol URI spellings use the opened document for preview edits", async () => {
        const text = "function f() -> int = 42\n";
        const document = await vscode.workspace.openTextDocument({
            language: "sail",
            content: text,
        });
        try {
            const uri = document.uri.toString();
            // Force a different spelling that VS Code resolves to this same
            // resource, independently of the host's path and operating system.
            const escaped = uri.replace("Untitled", "%55ntitled");
            assert.notEqual(escaped, uri);
            assert.equal(vscode.Uri.parse(escaped).toString(), uri);
            const edit = new vscode.WorkspaceEdit();
            edit.replace(document.uri, new vscode.Range(0, 22, 0, 24), "43");
            const previews = replacementPreviews(edit, [
                { base: { uri: escaped, version: document.version, text }, document },
            ]);
            assert.equal(previews.length, 1);
            assert.equal(previews[0].uri.toString(), uri);
            assert.equal(previews[0].count, 1);
            assert.equal(previews[0].before, text);
            assert.equal(previews[0].after, text.replace("42", "43"));
            assert.equal(
                document.getText(),
                text,
                "rendering a preview must not change its source",
            );
        } finally {
            await vscode.window.showTextDocument(document);
            await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor");
        }
    });
});
