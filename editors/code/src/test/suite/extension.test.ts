import * as assert from "node:assert/strict";
import * as vscode from "vscode";

interface ProjectGraphEntry {
    manifest: string;
    modules: unknown[];
}

interface ExtensionApi {
    test: {
        status(): { health: string; quiescent: boolean; message?: string } | undefined;
        projectGraph(): readonly ProjectGraphEntry[];
    };
}

async function eventually(message: string, predicate: () => boolean, timeout = 20_000) {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
        if (predicate()) return;
        await new Promise((resolve) => setTimeout(resolve, 50));
    }
    assert.fail(message);
}

suite("sail-lsp extension", () => {
    test("package manifest exposes bundled server setting", async () => {
        const extension = vscode.extensions.getExtension("Lunarwwww.sail-lsp");
        assert.ok(extension, "extension should be present in test host");
        const packageJson = extension.packageJSON as {
            contributes?: { configuration?: { properties?: Record<string, unknown> } };
        };
        assert.ok(packageJson.contributes?.configuration?.properties?.["sail-lsp.server.path"]);
    });

    test("real server drives status, graph, manifest completion, edits, and commands", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp");
        assert.ok(extension, "extension should be present in test host");
        const api = await extension.activate();
        await eventually(
            "server status never became ready",
            () => api.test.status()?.quiescent === true && api.test.status()?.health === "ok",
        );

        const commands = await vscode.commands.getCommands(true);
        for (const command of [
            "sail-lsp.showProjectGraph",
            "sail-lsp.openProjectManifest",
            "sail-lsp.parentModule",
            "sail-lsp.structuralSearch",
            "sail-lsp.recheckProject",
            "sail-lsp.cancelFlycheck",
            "sail-lsp.clearFlycheck",
        ]) {
            assert.ok(commands.includes(command), `${command} should be registered`);
        }

        await vscode.commands.executeCommand("sail-lsp.showProjectGraph");
        await eventually("project graph stayed empty", () => api.test.projectGraph().length === 1);
        assert.match(api.test.projectGraph()[0].manifest, /sail\.proj$/);

        const root = vscode.workspace.workspaceFolders?.[0]?.uri;
        assert.ok(root);
        const manifestUri = vscode.Uri.joinPath(root, "sail.proj");
        const manifest = await vscode.workspace.openTextDocument(manifestUri);
        const editor = await vscode.window.showTextDocument(manifest);
        const lastLine = manifest.lineCount;
        await editor.edit((edit) =>
            edit.insert(manifest.positionAt(manifest.getText().length), "Extra { files nes }\n"),
        );
        await eventually(
            "unsaved manifest status was not published",
            () => api.test.status()?.message?.includes("Unsaved project manifest") === true,
        );
        const completion = await vscode.commands.executeCommand<vscode.CompletionList>(
            "vscode.executeCompletionItemProvider",
            manifestUri,
            new vscode.Position(lastLine, "Extra { files nes".length),
        );
        assert.ok(
            completion.items.some((item) => item.label === "nested.sail"),
            `path completion missing: ${completion.items.map((item) => String(item.label)).join(", ")}`,
        );
        await vscode.commands.executeCommand("workbench.action.files.revert");
        await eventually(
            "reverting the manifest did not clear its status",
            () => !api.test.status()?.message?.includes("Unsaved project manifest"),
        );

        const unlinkedUri = vscode.Uri.joinPath(root, "unlinked.sail");
        const unlinked = await vscode.workspace.openTextDocument(unlinkedUri);
        await vscode.window.showTextDocument(unlinked);
        await eventually("unlinked-file diagnostic was not published", () =>
            vscode.languages.getDiagnostics(unlinkedUri).some((d) => d.code === "unlinked-file"),
        );
        const actions = await vscode.commands.executeCommand<
            (vscode.CodeAction | vscode.Command)[]
        >("vscode.executeCodeActionProvider", unlinkedUri, new vscode.Range(0, 0, 0, 1));
        const add = actions.find(
            (action): action is vscode.CodeAction =>
                "edit" in action && action.title.includes("Add") && action.edit !== undefined,
        );
        assert.ok(add?.edit, "unlinked-file action should expose a previewable WorkspaceEdit");
        assert.ok(add.edit.entries().some(([uri]) => uri.toString() === manifestUri.toString()));

        const sampleUri = vscode.Uri.joinPath(root, "sample.sail");
        await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(sampleUri));
        await vscode.commands.executeCommand("sail-lsp.openProjectManifest");
        assert.equal(
            vscode.window.activeTextEditor?.document.uri.toString(),
            manifestUri.toString(),
        );

        await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(sampleUri));
        await vscode.commands.executeCommand("sail-lsp.recheckProject");
        await vscode.commands.executeCommand("sail-lsp.cancelFlycheck");
        await vscode.commands.executeCommand("sail-lsp.clearFlycheck");
    });
    test("structural search shows real locations without changing the source", async () => {
        const root = vscode.workspace.workspaceFolders?.[0]?.uri;
        assert.ok(root);
        const uri = vscode.Uri.joinPath(root, "sample.sail");
        const document = await vscode.workspace.openTextDocument(uri);
        const editor = await vscode.window.showTextDocument(document);
        const before = document.getText();
        const version = document.version;
        const position = document.positionAt(before.indexOf("42"));
        editor.selection = new vscode.Selection(position, position);
        const found = await vscode.commands.executeCommand<vscode.Location[]>(
            "sail-lsp.structuralSearch",
            "42",
        );
        assert.equal(found.length, 1);
        assert.equal(found[0].uri.toString(), uri.toString());
        assert.equal(document.getText(found[0].range), "42");
        assert.equal(document.getText(), before);
        assert.equal(document.version, version);
        assert.deepEqual(await vscode.commands.executeCommand("sail-lsp.structuralSearch", ""), []);
    });
});
