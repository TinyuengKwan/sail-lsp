import * as assert from "node:assert/strict";
import * as vscode from "vscode";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import type { Runnable } from "../../runnables";

interface ProjectGraphEntry {
    manifest: string;
    modules: unknown[];
}

interface ExtensionApi {
    test: {
        status(): { health: string; quiescent: boolean; message?: string } | undefined;
        projectGraph(): readonly ProjectGraphEntry[];
        ssr: { pending(): number; stale(): boolean };
        runnables: { list(uri: vscode.Uri): Promise<Runnable[]>; activeTasks(): number };
    };
}

async function eventually(
    message: string | (() => string),
    predicate: () => boolean,
    timeout = 20_000,
) {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
        if (predicate()) return;
        await new Promise((resolve) => setTimeout(resolve, 50));
    }
    assert.fail(typeof message === "function" ? message() : message);
}

async function closeRestoredEditors() {
    assert.deepEqual(
        vscode.workspace.textDocuments
            .filter((doc) => doc.isDirty)
            .map((doc) => doc.uri.toString()),
        [],
        "preceding tests must restore their documents before closing their editors",
    );
    // Dispose embedded Peek editors as well as their owning editor views.
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    await eventually(
        "preceding editors did not close",
        () => vscode.window.visibleTextEditors.length === 0,
    );
}

suite("sail-lsp extension", () => {
    test("project tasks execute argv/context, expose failure, cancel and reject stale or dirty runs", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        const root = vscode.workspace.workspaceFolders![0].uri;
        const uri = vscode.Uri.joinPath(root, "sample.sail");
        const document = await vscode.workspace.openTextDocument(uri);
        await vscode.window.showTextDocument(document);
        const temp = fs.mkdtempSync(path.join(os.tmpdir(), "sail tasks "));
        const config = vscode.workspace.getConfiguration("sail-lsp");
        const keys = [
            "workspace.projectVariables",
            "workspace.projectModules",
            "runnables.tasks",
            "flycheck.command",
            "server.extraEnv",
        ];
        const saved = keys.map((key) => config.inspect(key)?.workspaceValue);
        const compiler = path.join(temp, "selected compiler with spaces");
        const outputs = ["ok", "fail", "wait"].map((name) => path.join(temp, `${name}.json`));
        const tasks = ["ok", "fail", "wait"].map((mode, index) => ({
            label: mode,
            kind: index === 0 ? "export" : "test",
            command: process.env.SAIL_LSP_TEST_NODE!,
            args: [
                vscode.Uri.joinPath(root, "tools", "task-probe.mjs").fsPath,
                mode,
                outputs[index],
                index === 0 ? "@environment" : "${projectContext}",
                "literal $(never-exec) && value with spaces",
            ],
            environment: {
                TASK_COMPILER: "${sail}",
                SAIL_LSP_PROJECT_CONTEXT: "${projectContext}",
            },
        }));
        tasks.push({
            label: "CLI only",
            kind: "export",
            command: "${sail}",
            args: ["${projectArgs}"],
            environment: { TASK_COMPILER: "${sail}", SAIL_LSP_PROJECT_CONTEXT: "" },
        });
        const fresh = async (): Promise<Runnable[]> => {
            const deadline = Date.now() + 20_000;
            while (Date.now() < deadline) {
                const entries = await api.test.runnables.list(uri);
                if (
                    entries.length === 5 &&
                    entries
                        .filter((entry) => !entry.label.startsWith("CLI only"))
                        .every((entry) => !entry.disabled)
                )
                    return entries;
                await new Promise((resolve) => setTimeout(resolve, 50));
            }
            throw new Error("Project tasks did not become available");
        };
        const execute = async (
            entry: Runnable,
            afterStart?: () => Promise<void>,
        ): Promise<number | undefined> => {
            let timer: NodeJS.Timeout | undefined;
            let subscription: vscode.Disposable | undefined;
            const ended = new Promise<number | undefined>((resolve, reject) => {
                timer = setTimeout(() => reject(new Error("Task did not exit")), 20_000);
                subscription = vscode.tasks.onDidEndTaskProcess((event) => {
                    if (event.execution.task.definition.id === entry.id) resolve(event.exitCode);
                });
            });
            try {
                await vscode.commands.executeCommand(
                    "sail-lsp.runProjectTask",
                    uri,
                    entry.id,
                    entry.fingerprint,
                );
                await afterStart?.();
                return await ended;
            } finally {
                clearTimeout(timer);
                subscription?.dispose();
            }
        };
        try {
            assert.ok(process.env.SAIL_LSP_TEST_NODE);
            await config.update(
                "workspace.projectVariables",
                { TYPED: "true", FLAG: false, LITERAL: "${workspaceFolder}" },
                vscode.ConfigurationTarget.Workspace,
            );
            await config.update(
                "workspace.projectModules",
                ["Main"],
                vscode.ConfigurationTarget.Workspace,
            );
            await config.update("flycheck.command", compiler, vscode.ConfigurationTarget.Workspace);
            await config.update(
                "server.extraEnv",
                { PROJECT_TASK_TEST_EXTRA: "inherited value" },
                vscode.ConfigurationTarget.Workspace,
            );
            await config.update("runnables.tasks", tasks, vscode.ConfigurationTarget.Workspace);
            const entries = await fresh();
            const selected = (mode: string) =>
                entries.find((entry) => entry.label.startsWith(`${mode} (`))!;
            const unsupported = selected("CLI only");
            assert.match(unsupported.disabled!, /projectContext/);
            await assert.rejects(
                async () =>
                    vscode.commands.executeCommand(
                        "sail-lsp.runProjectTask",
                        uri,
                        unsupported.id,
                        unsupported.fingerprint,
                    ),
                /projectContext/,
            );
            const lenses = await vscode.commands.executeCommand<vscode.CodeLens[]>(
                "vscode.executeCodeLensProvider",
                uri,
            );
            assert.ok(lenses?.some((lens) => lens.command?.command === "sail-lsp.runProjectTask"));
            assert.equal(await execute(selected("ok")), 0);
            const result = JSON.parse(fs.readFileSync(outputs[0], "utf8"));
            assert.equal(path.relative(result.cwd, root.fsPath), "");
            assert.deepEqual(result.context.variables, {
                FLAG: false,
                LITERAL: "${workspaceFolder}",
                TYPED: "true",
            });
            assert.deepEqual(result.context.modules, ["Main"]);
            assert.equal(result.context.compiler, compiler);
            assert.equal(result.compiler, compiler);
            assert.equal(result.inherited, "inherited value");
            assert.deepEqual(result.args, ["literal $(never-exec) && value with spaces"]);
            assert.equal(await execute(selected("fail")), 7);
            await execute(selected("wait"), async () => {
                await eventually(
                    "task actually started",
                    () => fs.existsSync(outputs[2]) && api.test.runnables.activeTasks() === 1,
                );
                await vscode.commands.executeCommand("sail-lsp.cancelProjectTasks");
            });
            await eventually(
                "task resources released",
                () => api.test.runnables.activeTasks() === 0,
            );
            const stopped = JSON.parse(fs.readFileSync(outputs[2], "utf8"));
            for (const pid of [stopped.pid, stopped.descendant]) {
                assert.equal(typeof pid, "number");
                await eventually("task process tree exited", () => {
                    try {
                        process.kill(pid, 0);
                        return false;
                    } catch (error) {
                        return (error as NodeJS.ErrnoException).code === "ESRCH";
                    }
                });
            }
            const editor = await vscode.window.showTextDocument(document);
            await editor.edit((edit) =>
                edit.insert(new vscode.Position(0, 0), "// dirty task input\n"),
            );
            await assert.rejects(
                async () =>
                    vscode.commands.executeCommand(
                        "sail-lsp.runProjectTask",
                        uri,
                        selected("ok").id,
                        selected("ok").fingerprint,
                    ),
                /Save .*before running/,
            );
            await vscode.commands.executeCommand("workbench.action.files.revert");
            const old = selected("ok");
            await config.update(
                "runnables.tasks",
                tasks.map((task) => ({ ...task, label: `${task.label} changed` })),
                vscode.ConfigurationTarget.Workspace,
            );
            const deadline = Date.now() + 20_000;
            while (Date.now() < deadline) {
                const entry = (await fresh()).find((entry) => entry.id === old.id)!;
                if (entry.fingerprint !== old.fingerprint) break;
                await new Promise((resolve) => setTimeout(resolve, 50));
            }
            await assert.rejects(
                async () =>
                    vscode.commands.executeCommand(
                        "sail-lsp.runProjectTask",
                        uri,
                        old.id,
                        old.fingerprint,
                    ),
                /task changed/,
            );
        } finally {
            await vscode.commands.executeCommand("sail-lsp.cancelProjectTasks");
            await vscode.window.showTextDocument(document);
            if (document.isDirty)
                await vscode.commands.executeCommand("workbench.action.files.revert");
            for (let index = 0; index < keys.length; index++)
                await config.update(
                    keys[index],
                    saved[index],
                    vscode.ConfigurationTarget.Workspace,
                );
            fs.rmSync(temp, { recursive: true, force: true });
        }
    });

    test("exports a diagnostic report through the real server and command", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        const root = vscode.workspace.workspaceFolders![0].uri;
        const document = await vscode.workspace.openTextDocument(
            vscode.Uri.joinPath(root, "sample.sail"),
        );
        await vscode.window.showTextDocument(document);
        const target = vscode.Uri.joinPath(root, "diagnostic-report-test.json");
        try {
            await vscode.commands.executeCommand("sail-lsp.exportDiagnosticReport", target);
            const report = JSON.parse(
                Buffer.from(await vscode.workspace.fs.readFile(target)).toString(),
            );
            assert.equal(report.schemaVersion, 1);
            assert.equal(report.server.schemaVersion, 1);
            assert.equal(report.server.activeFile.uri, document.uri.toString());
            assert.equal(report.server.workspace.loaded, true);
            assert.ok(report.server.nativeAnalysis.failures);
            assert.ok(report.server.build.stdlibRevision);
            assert.ok(!JSON.stringify(report).includes(document.getText().trim()));
            assert.equal(report.client.extraEnv, undefined);
        } finally {
            await vscode.workspace.fs.delete(target, { useTrash: false });
        }
    });

    test("SSR preview is cancellable, rejects stale sources, applies and re-diagnoses", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        const uri = vscode.Uri.joinPath(vscode.workspace.workspaceFolders![0].uri, "sample.sail");
        const document = await vscode.workspace.openTextDocument(uri);
        const original = document.getText();
        const show = async () => {
            const editor = await vscode.window.showTextDocument(document);
            editor.selection = new vscode.Selection(1, 37, 1, 37);
            return editor;
        };
        try {
            await show();
            const count = await vscode.commands.executeCommand<number>(
                "sail-lsp.structuralReplace",
                "42 ==>> 43",
            );
            assert.equal(count, 1);
            assert.equal(document.getText(), original);
            assert.equal(api.test.ssr.pending(), 1);
            await vscode.commands.executeCommand("sail-lsp.cancelStructuralReplace");
            assert.equal(api.test.ssr.pending(), 0);
            assert.equal(document.getText(), original);
            const editor = await show();
            await vscode.commands.executeCommand("sail-lsp.structuralReplace", "42 ==>> 43");
            await editor.edit((edit) =>
                edit.insert(new vscode.Position(0, 0), "// edited after preview\n"),
            );
            await eventually("preview invalidated", () => api.test.ssr.stale());
            assert.equal(
                await vscode.commands.executeCommand("sail-lsp.applyStructuralReplace"),
                false,
            );
            assert.ok(document.getText().includes("= 42"));
            await vscode.commands.executeCommand("sail-lsp.cancelStructuralReplace");
            await vscode.commands.executeCommand("workbench.action.files.revert");
            await show();
            assert.equal(
                await vscode.commands.executeCommand("sail-lsp.structuralReplace", "42 ==>> true"),
                1,
            );
            assert.equal(
                await vscode.commands.executeCommand("sail-lsp.applyStructuralReplace"),
                true,
            );
            assert.ok(document.getText().includes("true"));
            await eventually("replacement was re-diagnosed", () =>
                vscode.languages
                    .getDiagnostics(uri)
                    .some((d) => d.severity === vscode.DiagnosticSeverity.Error),
            );
        } finally {
            await vscode.commands.executeCommand("sail-lsp.cancelStructuralReplace");
            await vscode.window.showTextDocument(document);
            await vscode.commands.executeCommand("workbench.action.files.revert");
            assert.equal(document.getText(), original);
        }
    });

    test("graph refreshes automatically and module navigation uses the unsaved manifest", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("initial graph installed", () => api.test.projectGraph().length === 1);
        const uri = vscode.Uri.joinPath(vscode.workspace.workspaceFolders![0].uri, "sail.proj");
        const document = await vscode.workspace.openTextDocument(uri);
        const original = document.getText();
        try {
            const editor = await vscode.window.showTextDocument(document);
            await editor.edit((edit) =>
                edit.insert(new vscode.Position(0, 0), "// unsaved line\n"),
            );
            await vscode.commands.executeCommand(
                "sail-lsp.openGraphModule",
                uri.toString(),
                "Main",
            );
            assert.equal(vscode.window.activeTextEditor?.selection.start.line, 1);
            await editor.edit((edit) =>
                edit.insert(document.positionAt(document.getText().length), "Extra { }\n"),
            );
            await document.save();
            await eventually(
                "graph refreshed after save",
                () => api.test.projectGraph()[0].modules.length === 2,
            );
        } finally {
            const editor = await vscode.window.showTextDocument(document);
            await editor.edit((edit) =>
                edit.replace(
                    new vscode.Range(
                        document.positionAt(0),
                        document.positionAt(document.getText().length),
                    ),
                    original,
                ),
            );
            await document.save();
            await eventually(
                "graph restored",
                () => api.test.projectGraph()[0].modules.length === 1,
            );
        }
    });

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
            "sail-lsp.viewIncludeGraph",
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

        await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(sampleUri));
        await vscode.commands.executeCommand("sail-lsp.viewIncludeGraph");
        // Shown beside the source without taking focus; the fixture has no
        // `$include`, which the renderer says in so many words.
        const isGraph = (editor: vscode.TextEditor) =>
            editor.document.languageId === "plaintext" &&
            editor.document.getText().startsWith("(no include relationships)");
        await eventually("include graph was not shown", () =>
            vscode.window.visibleTextEditors.some(isGraph),
        );
        // An untitled document with content counts as dirty; later tests
        // expect none left behind.
        await vscode.window.showTextDocument(
            vscode.window.visibleTextEditors.find(isGraph)!.document,
        );
        await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor");
    });
    test("structural search shows real locations without changing the source", async () => {
        await closeRestoredEditors();
        const root = vscode.workspace.workspaceFolders?.[0]?.uri;
        assert.ok(root);
        const uri = vscode.Uri.joinPath(root, "sample.sail");
        const document = await vscode.workspace.openTextDocument(uri);
        const before = document.getText();
        const version = document.version;
        const position = document.positionAt(before.indexOf("42"));
        const editor = await vscode.window.showTextDocument(document, {
            preserveFocus: false,
            preview: false,
            selection: new vscode.Range(position, position),
        });
        await eventually(
            "structural search editor did not activate",
            () =>
                vscode.window.activeTextEditor === editor &&
                editor.selection.isEmpty &&
                editor.selection.active.isEqual(position),
        );
        try {
            const found = await vscode.commands.executeCommand<vscode.Location[]>(
                "sail-lsp.structuralSearch",
                "42",
            );
            assert.equal(found.length, 1);
            assert.equal(found[0].uri.toString(), uri.toString());
            assert.equal(document.getText(found[0].range), "42");
            assert.equal(document.getText(), before);
            assert.equal(document.version, version);
            assert.deepEqual(
                await vscode.commands.executeCommand("sail-lsp.structuralSearch", ""),
                [],
            );
        } finally {
            await vscode.commands.executeCommand("closeReferenceSearch");
        }
    });
    test("formatting uses actual typing options and opens a read-only explanation", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        await closeRestoredEditors();
        const uri = vscode.Uri.joinPath(vscode.workspace.workspaceFolders![0].uri, "sample.sail");
        const document = await vscode.workspace.openTextDocument(uri);
        const original = document.getText();
        let editor = await vscode.window.showTextDocument(document);
        const oldOptions = { ...editor.options };
        const config = vscode.workspace.getConfiguration("editor", document);
        const workspaceConfig = vscode.workspace.getConfiguration();
        const originalLanguageSettings = workspaceConfig.inspect("[sail]")?.workspaceValue;
        const configure = (key: string, value: unknown) =>
            config.update(key, value, vscode.ConfigurationTarget.Workspace, true);
        const source = "function f() = {\n  1\n}\n";
        const replace = async (text: string) => {
            const edit = new vscode.WorkspaceEdit();
            edit.replace(
                uri,
                new vscode.Range(
                    document.positionAt(0),
                    document.positionAt(document.getText().length),
                ),
                text,
            );
            assert.ok(await vscode.workspace.applyEdit(edit));
        };
        try {
            await configure("formatOnType", true);
            await configure("autoIndent", "none");
            await configure("detectIndentation", false);
            await configure("tabSize", 4);
            for (const spaces of [true, false]) {
                await replace(source);
                // Unlike the optimistic editor.options setter, this observation
                // waits for the editor model to report the applied configuration.
                await configure("insertSpaces", spaces);
                await eventually(
                    () => `editor options did not settle: ${JSON.stringify(editor.options)}`,
                    () => editor.options.tabSize === 4 && editor.options.insertSpaces === spaces,
                );
                // Activate this document and set its selection in one awaited
                // workbench operation, rather than targeting whichever editor
                // currently has DOM focus after the preceding Peek test.
                editor = await vscode.window.showTextDocument(document, {
                    viewColumn: editor.viewColumn,
                    preserveFocus: false,
                    preview: false,
                    selection: new vscode.Range(
                        0,
                        "function f() = {".length,
                        0,
                        "function f() = {".length,
                    ),
                });
                await eventually(
                    () =>
                        `typing cursor did not settle: ${JSON.stringify({
                            selection: editor.selection,
                            activeEditor: vscode.window.activeTextEditor?.document.uri.toString(),
                            activeSelection: vscode.window.activeTextEditor?.selection,
                            activeMatches: vscode.window.activeTextEditor === editor,
                            text: document.getText(),
                        })}`,
                    () =>
                        vscode.window.activeTextEditor === editor &&
                        editor.selection.isEmpty &&
                        editor.selection.active.isEqual(
                            new vscode.Position(0, "function f() = {".length),
                        ),
                );
                const version = document.version;
                await vscode.commands.executeCommand("type", { text: "\n" });
                await eventually(
                    "typing did not change the document",
                    () => document.version > version,
                );
                assert.equal(document.lineCount, 5, "typing must insert one newline");
                assert.equal(
                    document.lineAt(3).text,
                    "}",
                    `typing targeted the wrong line: ${JSON.stringify({
                        selection: editor.selection,
                        activeSelection: vscode.window.activeTextEditor?.selection,
                        text: document.getText(),
                    })}`,
                );
                await eventually(
                    () =>
                        `on-type indentation must use the editor options: ${JSON.stringify({
                            expected: spaces ? "    " : "\t",
                            options: editor.options,
                            selection: editor.selection,
                            activeEditor: vscode.window.activeTextEditor?.document.uri.toString(),
                            text: document.getText(),
                        })}`,
                    () => document.lineAt(1).text === (spaces ? "    " : "\t"),
                );
                assert.equal(document.lineAt(2).text, "  1", "typing changed another line");
            }
            editor.selection = new vscode.Selection(0, 0, 1, 0);
            await vscode.commands.executeCommand("sail-lsp.explainFormatting");
            const preview = vscode.window.activeTextEditor!.document;
            assert.equal(preview.uri.scheme, "sail-preview");
            const report = JSON.parse(preview.getText());
            assert.equal(report.options.tabSize, 4);
            assert.equal(report.options.insertSpaces, false);
            assert.equal(report.styleVersion, 1);
            assert.equal(report.status, "range_boundary");
        } finally {
            await replace(original);
            await vscode.window.showTextDocument(document);
            await workspaceConfig.update(
                "[sail]",
                originalLanguageSettings,
                vscode.ConfigurationTarget.Workspace,
            );
            editor.options = oldOptions;
        }
    });

    test("a generate-function assist arrives as a snippet that selects the stub's placeholder", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        const uri = vscode.Uri.joinPath(vscode.workspace.workspaceFolders![0].uri, "sample.sail");
        const document = await vscode.workspace.openTextDocument(uri);
        const original = document.getText();
        const source = "val f : int -> int\nfunction f(x) = helper(x)\n";
        const replace = async (text: string) => {
            const edit = new vscode.WorkspaceEdit();
            edit.replace(
                uri,
                new vscode.Range(
                    document.positionAt(0),
                    document.positionAt(document.getText().length),
                ),
                text,
            );
            assert.ok(await vscode.workspace.applyEdit(edit));
        };
        try {
            await replace(source);
            const editor = await vscode.window.showTextDocument(document);
            const at = document.positionAt(source.indexOf("helper"));
            editor.selection = new vscode.Selection(at, at);
            // Resolved through the client, so the edit reaches VS Code as a
            // `SnippetTextEdit` rather than as text with a literal `$0`.
            let generate: vscode.CodeAction | undefined;
            const deadline = Date.now() + 20_000;
            while (!generate?.edit && Date.now() < deadline) {
                const actions = await vscode.commands.executeCommand<
                    (vscode.CodeAction | vscode.Command)[]
                >("vscode.executeCodeActionProvider", uri, new vscode.Range(at, at), undefined, 20);
                generate = actions.find(
                    (action): action is vscode.CodeAction =>
                        "kind" in action && action.title === "Generate function `helper`",
                );
                if (!generate?.edit) await new Promise((resolve) => setTimeout(resolve, 50));
            }
            assert.ok(generate?.edit, "the assist should be offered with a resolved edit");
            assert.ok(await vscode.workspace.applyEdit(generate.edit));
            await eventually(
                () => `stub not inserted: ${document.getText()}`,
                () => document.getText().includes("val helper : _ -> _"),
            );
            assert.ok(!document.getText().includes("$"), document.getText());
            // `${1:_}`, the return type, is what the user types over first.
            await eventually(
                () => `placeholder not selected: ${JSON.stringify(editor.selection)}`,
                () =>
                    editor.selection.start.line === 4 &&
                    document.getText(editor.selection) === "_" &&
                    document.lineAt(4).text.startsWith("val helper : _ -> "),
            );
        } finally {
            await replace(original);
            await vscode.window.showTextDocument(document);
        }
    });

    test("editing commands find the matching brace, move items, join lines and complete Enter", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        const uri = vscode.Uri.joinPath(vscode.workspace.workspaceFolders![0].uri, "sample.sail");
        const document = await vscode.workspace.openTextDocument(uri);
        const original = document.getText();
        const source = "function a() -> int = {\n  1\n}\nfunction b() -> int = 2\n/// doc\n";
        const replace = async (text: string) => {
            const edit = new vscode.WorkspaceEdit();
            edit.replace(
                uri,
                new vscode.Range(
                    document.positionAt(0),
                    document.positionAt(document.getText().length),
                ),
                text,
            );
            assert.ok(await vscode.workspace.applyEdit(edit));
        };
        const select = (position: vscode.Position) => {
            editor.selection = new vscode.Selection(position, position);
        };
        let editor: vscode.TextEditor;
        try {
            await replace(source);
            editor = await vscode.window.showTextDocument(document);
            select(document.positionAt(source.indexOf("{")));
            await vscode.commands.executeCommand("sail-lsp.matchingBrace");
            assert.deepEqual(
                [editor.selection.active.line, editor.selection.active.character],
                [2, 0],
                "the cursor moves to the closing brace",
            );
            const lineOf = (text: string) => {
                const lines = document.getText().split("\n");
                const line = lines.indexOf(text);
                assert.notEqual(line, -1, `${JSON.stringify(text)} is not a line of ${lines}`);
                return line;
            };
            select(new vscode.Position(0, 0));
            await vscode.commands.executeCommand("sail-lsp.moveItemDown");
            await eventually(
                () => `item not moved: ${document.getText()}`,
                () => {
                    const text = document.getText();
                    return (
                        text.indexOf("function b()") < text.indexOf("function a()") &&
                        text.split("\n").sort().join("\n") === source.split("\n").sort().join("\n")
                    );
                },
            );
            const opening = lineOf("function a() -> int = {");
            select(new vscode.Position(opening, 0));
            await vscode.commands.executeCommand("sail-lsp.joinLines");
            await eventually(
                () => `lines not joined: ${document.getText()}`,
                () =>
                    document.lineCount === 5 &&
                    document.lineAt(opening).text.startsWith("function a() -> int = {") &&
                    document.lineAt(opening).text.endsWith("1"),
            );
            // Enter after a doc comment continues it; after plain code it is
            // a newline and nothing else.
            const doc = lineOf("/// doc");
            select(document.lineAt(doc).range.end);
            await vscode.commands.executeCommand("sail-lsp.onEnter");
            await eventually(
                () => `doc comment not continued: ${JSON.stringify(document.getText())}`,
                () =>
                    document.lineAt(doc).text === "/// doc" &&
                    document.lineAt(doc + 1).text === "/// " &&
                    editor.selection.active.isEqual(new vscode.Position(doc + 1, 4)),
            );
            const before = document.lineCount;
            const plain = lineOf("function b() -> int = 2");
            select(document.lineAt(plain).range.end);
            await vscode.commands.executeCommand("sail-lsp.onEnter");
            await eventually(
                () => `newline not typed: ${JSON.stringify(document.getText())}`,
                () => document.lineCount === before + 1 && document.lineAt(plain + 1).text === "",
            );
            await new Promise((resolve) => setTimeout(resolve, 300));
            assert.equal(document.lineAt(plain + 1).text, "", "plain code gained a continuation");
        } finally {
            await replace(original);
            await vscode.window.showTextDocument(document);
        }
    });

    test("reference lenses run the client's showReferences command and task lenses obey their setting", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        const uri = vscode.Uri.joinPath(vscode.workspace.workspaceFolders![0].uri, "sample.sail");
        const document = await vscode.workspace.openTextDocument(uri);
        const original = document.getText();
        const source =
            "val helper : int -> int\nfunction helper(value) = value\nfunction main() -> int = helper(1)\n";
        const replace = async (text: string) => {
            const edit = new vscode.WorkspaceEdit();
            edit.replace(
                uri,
                new vscode.Range(
                    document.positionAt(0),
                    document.positionAt(document.getText().length),
                ),
                text,
            );
            assert.ok(await vscode.workspace.applyEdit(edit));
        };
        const config = vscode.workspace.getConfiguration("sail-lsp");
        const savedShowCodeLens = config.inspect("runnables.showCodeLens")?.workspaceValue;
        const lenses = async () =>
            (await vscode.commands.executeCommand<vscode.CodeLens[]>(
                "vscode.executeCodeLensProvider",
                uri,
                50,
            )) ?? [];
        try {
            await replace(source);
            await vscode.window.showTextDocument(document);
            let references: vscode.CodeLens | undefined;
            const deadline = Date.now() + 20_000;
            while (!references && Date.now() < deadline) {
                references = (await lenses()).find(
                    (lens) =>
                        lens.command?.command === "sail-lsp.showReferences" &&
                        lens.command.title === "1 reference",
                );
                if (!references) await new Promise((resolve) => setTimeout(resolve, 50));
            }
            assert.ok(references?.command, "a resolved reference lens should carry the command");
            // The arguments are protocol values; the command converts them.
            assert.equal(typeof references.command.arguments?.[0], "string");
            await vscode.commands.executeCommand(
                references.command.command,
                ...references.command.arguments!,
            );
            await vscode.commands.executeCommand("closeReferenceSearch");
            assert.ok(
                (await lenses()).some(
                    (lens) => lens.command?.command === "sail-lsp.runProjectTask",
                ),
                "task lenses are shown by default",
            );
            await config.update(
                "runnables.showCodeLens",
                false,
                vscode.ConfigurationTarget.Workspace,
            );
            assert.ok(
                (await lenses()).every(
                    (lens) => lens.command?.command !== "sail-lsp.runProjectTask",
                ),
                "runnables.showCodeLens=false hides the task lenses",
            );
        } finally {
            await config.update(
                "runnables.showCodeLens",
                savedShowCodeLens,
                vscode.ConfigurationTarget.Workspace,
            );
            await replace(original);
            await vscode.window.showTextDocument(document);
        }
    });

    test("analysis explanations and read-only preprocessing previews expose actual source links", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        const root = vscode.workspace.workspaceFolders![0].uri;
        const uri = vscode.Uri.joinPath(root, "sample.sail");
        const document = await vscode.workspace.openTextDocument(uri);
        const original = document.getText();
        const source = "val identity : int -> int\nfunction use() -> int = identity(true)\n";
        const replace = async (text: string) => {
            const edit = new vscode.WorkspaceEdit();
            edit.replace(
                uri,
                new vscode.Range(
                    document.positionAt(0),
                    document.positionAt(document.getText().length),
                ),
                text,
            );
            assert.ok(await vscode.workspace.applyEdit(edit));
        };
        try {
            await replace(source);
            let editor = await vscode.window.showTextDocument(document);
            const at = document.positionAt(source.indexOf("true"));
            editor.selection = new vscode.Selection(at, at);
            await vscode.commands.executeCommand("sail-lsp.explainAnalysis");
            const explanation = vscode.window.activeTextEditor!.document;
            assert.equal(explanation.uri.scheme, "sail-preview");
            assert.ok(explanation.getText().includes("Expected: int"), explanation.getText());
            assert.ok(explanation.getText().includes("Actual: bool"), explanation.getText());
            const links = await vscode.commands.executeCommand<vscode.DocumentLink[]>(
                "vscode.executeLinkProvider",
                explanation.uri,
            );
            assert.ok(
                links?.some(
                    (link) => link.target?.with({ fragment: "" }).toString() === uri.toString(),
                ),
                JSON.stringify(links),
            );
            editor = await vscode.window.showTextDocument(document);
            editor.selection = new vscode.Selection(at, at);
            await vscode.commands.executeCommand("sail-lsp.expandInclude");
            const preview = vscode.window.activeTextEditor!.document;
            assert.equal(preview.uri.scheme, "sail-preview");
            assert.ok(preview.getText().includes(source), preview.getText());
            const sourceLinks = await vscode.commands.executeCommand<vscode.DocumentLink[]>(
                "vscode.executeLinkProvider",
                preview.uri,
            );
            assert.ok(
                sourceLinks?.some(
                    (link) => link.target?.with({ fragment: "" }).toString() === uri.toString(),
                ),
                JSON.stringify(sourceLinks),
            );
            assert.equal(vscode.workspace.fs.isWritableFileSystem(preview.uri.scheme), undefined);
        } finally {
            await replace(original);
            await vscode.window.showTextDocument(document);
        }
    });
});
