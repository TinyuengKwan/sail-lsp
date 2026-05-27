import * as assert from "node:assert/strict";
import * as path from "node:path";
import * as vscode from "vscode";

suite("sail-lsp extension", () => {
    test("package manifest exposes bundled server setting", async () => {
        const extension = vscode.extensions.getExtension("sail-lsp.sail-lsp");
        assert.ok(extension, "extension should be present in test host");

        const packageJson = extension.packageJSON as {
            contributes?: {
                configuration?: {
                    properties?: Record<string, unknown>;
                };
            };
        };

        assert.ok(packageJson.contributes?.configuration?.properties?.["sail-lsp.server.path"]);
    });

    test("activation fails gracefully without server binary", async () => {
        const extension = vscode.extensions.getExtension("sail-lsp.sail-lsp");
        assert.ok(extension, "extension should be present in test host");

        const extensionPath = extension.extensionPath;
        const serverDir = path.join(extensionPath, "server");
        const workspaceConfig = vscode.workspace.getConfiguration("sail-lsp");

        await workspaceConfig.update(
            "server.path",
            path.join(serverDir, "definitely-missing-sail-lsp"),
            vscode.ConfigurationTarget.Global,
        );

        await extension.activate();
        assert.ok(extension.isActive, "extension should still activate cleanly");

        await workspaceConfig.update("server.path", undefined, vscode.ConfigurationTarget.Global);
    });
});
