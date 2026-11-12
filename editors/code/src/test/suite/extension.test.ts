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
        testExplorer: {
            tests(): string[];
            run(ids: string[]): Promise<Record<string, "passed" | "failed" | "errored">>;
        };
    };
}

/** The upstream compiler, when the machine has one; interpreter runs need it. */
function sailOnPath(): string | undefined {
    for (const dir of (process.env["PATH"] ?? "").split(path.delimiter)) {
        const candidate = path.join(dir, process.platform === "win32" ? "sail.exe" : "sail");
        try {
            fs.accessSync(candidate, fs.constants.X_OK);
            return candidate;
        } catch {
            // continue
        }
    }
    return undefined;
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

/** `eventually` for a check that must be awaited and not overlapped. */
async function settles(
    message: string | (() => string),
    check: () => Promise<boolean>,
    timeout = 20_000,
) {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
        if (await check()) return;
        await new Promise((resolve) => setTimeout(resolve, 200));
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

/** Await the workbench selection and keyboard focus before issuing typing commands. */
async function focusSource(document: vscode.TextDocument, position: vscode.Position) {
    const editor = await vscode.window.showTextDocument(document, {
        preserveFocus: false,
        preview: false,
        selection: new vscode.Range(position, position),
    });
    await vscode.commands.executeCommand("workbench.action.focusActiveEditorGroup");
    await eventually(
        () =>
            `source cursor did not settle: ${JSON.stringify({
                uri: document.uri.toString(),
                selection: editor.selection,
                active: vscode.window.activeTextEditor?.document.uri.toString(),
            })}`,
        () =>
            vscode.window.activeTextEditor === editor &&
            editor.selection.isEmpty &&
            editor.selection.active.isEqual(position),
    );
    return editor;
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

    test("test functions get run lenses, the Test Explorer lists and runs them, and sail tasks are offered", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        const root = vscode.workspace.workspaceFolders![0].uri;
        const uri = vscode.Uri.joinPath(root, "sample.sail");
        const document = await vscode.workspace.openTextDocument(uri);
        const original = document.getText();
        const source =
            "val helper : unit -> unit\nfunction helper() = ()\n$[test]\nfunction test_first() -> unit = helper()\n$[test]\nfunction test_second() -> unit = ()\nfunction main() -> unit = test_first()\n";
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
        const keys = ["testExplorer", "runnables.tasks"];
        const saved = keys.map((key) => config.inspect(key)?.workspaceValue);
        const temp = fs.mkdtempSync(path.join(os.tmpdir(), "sail tasks "));
        const lenses = async () =>
            (await vscode.commands.executeCommand<vscode.CodeLens[]>(
                "vscode.executeCodeLensProvider",
                uri,
            )) ?? [];
        try {
            await replace(source);
            await document.save();
            await vscode.window.showTextDocument(document);
            await config.update(
                "runnables.tasks",
                [
                    {
                        label: "Export C",
                        kind: "export",
                        command: "${sail}",
                        args: ["-c", "${projectArgs}"],
                    },
                ],
                vscode.ConfigurationTarget.Workspace,
            );
            // A run lens on each `$[test]` function and on `main`, where it is.
            let placed: vscode.CodeLens[] = [];
            await settles(
                () => `run lenses missing: ${JSON.stringify(placed.map((l) => l.command?.title))}`,
                async () => {
                    placed = (await lenses()).filter(
                        (lens) =>
                            lens.command?.command === "sail-lsp.runProjectTask" &&
                            lens.command.title.startsWith("▶"),
                    );
                    return placed.length === 3;
                },
            );
            assert.deepEqual(
                placed.map((lens) => [lens.command!.title, lens.range.start.line]),
                [
                    ["▶ Run test test_first", 3],
                    ["▶ Run test test_second", 5],
                    ["▶ Run main", 6],
                ],
            );
            // The Test Explorer, once enabled, lists the tests of the open file
            // and not `main`.
            await config.update("testExplorer", true, vscode.ConfigurationTarget.Workspace);
            await eventually(
                () => `tests not listed: ${JSON.stringify(api.test.testExplorer.tests())}`,
                () => api.test.testExplorer.tests().length === 2,
            );
            const ids = api.test.testExplorer.tests().sort();
            assert.ok(
                ids[0].endsWith("#run-test_first") && ids[1].endsWith("#run-test_second"),
                ids.join(),
            );
            if (sailOnPath()) {
                // The verdict is the interpreter's: both evaluate to `()`.
                const verdicts = await api.test.testExplorer.run([]);
                assert.deepEqual(
                    Object.values(verdicts),
                    ["passed", "passed"],
                    JSON.stringify(verdicts),
                );
            } else {
                console.log("no `sail` on PATH: not running the tests");
            }
            // The task system sees the project task and the entries of the
            // open file, as `type: "sail"` tasks with the Sail problem matchers.
            let tasks: vscode.Task[] = [];
            await settles(
                () => `sail tasks missing: ${JSON.stringify(tasks.map((t) => t.name))}`,
                async () => {
                    tasks = await vscode.tasks.fetchTasks({ type: "sail" });
                    return (
                        tasks.some((task) => task.name.startsWith("Export C")) &&
                        tasks.some((task) => task.name === "Run test test_first") &&
                        tasks.some((task) => task.name === "Run main")
                    );
                },
            );
            const exportTask = tasks.find((task) => task.name.startsWith("Export C"))!;
            assert.equal(exportTask.definition.type, "sail");
            assert.equal(typeof exportTask.definition.command, "string");
            assert.deepEqual(exportTask.problemMatchers, ["$sail", "$sail-warning"]);
            assert.equal(exportTask.group, vscode.TaskGroup.Build);
            const testTask = tasks.find((task) => task.name === "Run test test_first")!;
            assert.equal(testTask.definition.script, "test_first()\n:run\n:quit\n");
            assert.equal(testTask.group, vscode.TaskGroup.Test);
            // The fixture's `tasks.json` entry is resolved by the provider and
            // runs as written, with VS Code's variables substituted.
            const output = path.join(process.env.SAIL_LSP_TEST_TMP!, "sail-lsp-user-task.json");
            fs.rmSync(output, { force: true });
            const written = tasks.find((task) => task.name === "user task");
            assert.ok(
                written?.execution,
                `tasks.json entry not resolved: ${tasks.map((t) => t.name)}`,
            );
            const exited = new Promise<number | undefined>((resolve) => {
                const subscription = vscode.tasks.onDidEndTaskProcess((event) => {
                    if (event.execution.task.name === "user task") {
                        subscription.dispose();
                        resolve(event.exitCode);
                    }
                });
            });
            await vscode.tasks.executeTask(written);
            assert.equal(await exited, 0);
            const probe = JSON.parse(fs.readFileSync(output, "utf8"));
            assert.deepEqual(probe.args, ["written in tasks.json"]);
            assert.equal(path.relative(probe.cwd, root.fsPath), "");
            fs.rmSync(output, { force: true });
        } finally {
            for (let index = 0; index < keys.length; index++)
                await config.update(
                    keys[index],
                    saved[index],
                    vscode.ConfigurationTarget.Workspace,
                );
            await replace(original);
            await document.save();
            await vscode.window.showTextDocument(document);
            fs.rmSync(temp, { recursive: true, force: true });
        }
    });

    test("changing server.path asks for a restart, and a restart recovers from a failed start", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        const root = vscode.workspace.workspaceFolders![0].uri;
        const uri = vscode.Uri.joinPath(root, "sample.sail");
        const config = vscode.workspace.getConfiguration("sail-lsp");
        const savedPath = config.inspect("server.path")?.workspaceValue;
        const prompts: string[] = [];
        const original = vscode.window.showInformationMessage;
        // The prompt, answered as a user clicking "Restart now" would.
        (vscode.window as { showInformationMessage: unknown }).showInformationMessage = (
            message: string,
            ...items: string[]
        ) => {
            prompts.push(message);
            return Promise.resolve(items.find((item) => item === "Restart now"));
        };
        try {
            await config.update(
                "server.path",
                path.join(os.tmpdir(), "sail-lsp-missing-binary"),
                vscode.ConfigurationTarget.Workspace,
            );
            await eventually(
                () => `no restart prompt: ${JSON.stringify(prompts)}`,
                () => prompts.some((message) => message.includes('"sail-lsp.server.path"')),
            );
            // The restart re-resolves the binary: this one cannot start.
            await eventually(
                () => `start did not fail: ${JSON.stringify(api.test.status())}`,
                () =>
                    api.test.status()?.health === "error" &&
                    api.test.status()?.message === "The server failed to start.",
            );
            assert.deepEqual(await api.test.runnables.list(uri), [], "no client, no runnables");
            // Restoring the path prompts again, and the restart creates a new
            // client although the previous start failed.
            await config.update("server.path", savedPath, vscode.ConfigurationTarget.Workspace);
            await eventually("no second prompt", () => prompts.length >= 2);
            await eventually(
                () => `server did not come back: ${JSON.stringify(api.test.status())}`,
                () => api.test.status()?.quiescent === true && api.test.status()?.health === "ok",
            );
            await settles(
                "the recreated client answers no runnables",
                async () => (await api.test.runnables.list(uri)).length >= 1,
            );
        } finally {
            (vscode.window as { showInformationMessage: unknown }).showInformationMessage =
                original;
            await config.update("server.path", savedPath, vscode.ConfigurationTarget.Workspace);
            if (api.test.status()?.health !== "ok") {
                await vscode.commands.executeCommand("sail-lsp.restartServer");
            }
            await eventually("server not restored", () => api.test.status()?.quiescent === true);
        }
    });

    test("syntax tree and item tree views follow the source and reveal the node under the cursor", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        await closeRestoredEditors();
        const uri = vscode.Uri.joinPath(vscode.workspace.workspaceFolders![0].uri, "sample.sail");
        const document = await vscode.workspace.openTextDocument(uri);
        const original = document.getText();
        try {
            await vscode.window.showTextDocument(document);
            // Views open beside the source and leave the focus on it.
            const shown = async (scheme: string) => {
                let found: vscode.TextDocument | undefined;
                await eventually(`no ${scheme} view is visible`, () => {
                    found = vscode.window.visibleTextEditors.find(
                        (editor) => editor.document.uri.scheme === scheme,
                    )?.document;
                    return found !== undefined;
                });
                return found!;
            };
            await vscode.commands.executeCommand("sail-lsp.showSyntaxTree");
            const tree = await shown("sail-lsp-syntax-tree");
            assert.equal(tree.uri.query, uri.toString());
            assert.match(tree.getText(), /^SOURCE_FILE@0\.\.\d+\n/);
            const before = tree.getText();
            await vscode.window.showTextDocument(document);
            await vscode.commands.executeCommand("sail-lsp.viewItemTree");
            const items = await shown("sail-lsp-item-tree");
            const itemsBefore = items.getText();
            assert.ok(itemsBefore.includes("structural_sample"), itemsBefore);
            // An edit to the source reaches both views without reopening them.
            const editor = await vscode.window.showTextDocument(document);
            await editor.edit((edit) =>
                edit.insert(document.positionAt(original.length), "val added_later : int\n"),
            );
            await eventually(
                () => `syntax tree view did not follow the edit: ${tree.getText()}`,
                () => tree.getText() !== before && tree.getText().includes('"added_later"'),
            );
            await eventually(
                () => `item tree view did not follow the edit: ${items.getText()}`,
                () => items.getText() !== itemsBefore && items.getText().includes("added_later"),
            );
            // A node line reveals its range in the source.
            const lines = tree.getText().split("\n");
            const line = lines.findIndex(
                (text) => text.includes("IDENT@") && text.endsWith('"structural_sample"'),
            );
            assert.notEqual(line, -1, tree.getText());
            assert.equal(
                await vscode.commands.executeCommand(
                    "sail-lsp.syntaxTreeReveal",
                    tree.uri.toString(),
                    line,
                ),
                true,
            );
            const source = vscode.window.visibleTextEditors.find(
                (candidate) => candidate.document.uri.toString() === uri.toString(),
            )!;
            assert.equal(source.document.getText(source.selection), "structural_sample");
            assert.equal(
                await vscode.commands.executeCommand(
                    "sail-lsp.syntaxTreeReveal",
                    tree.uri.toString(),
                    0,
                ),
                true,
            );
            assert.equal(source.document.getText(source.selection), document.getText());
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
            return focusSource(document, new vscode.Position(1, 37));
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
            await vscode.window.showTextDocument(document);
            await vscode.commands.executeCommand("workbench.action.files.revert");
            await eventually("SSR source did not revert", () => document.getText() === original);
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
            await eventually("SSR source was not restored", () => document.getText() === original);
            assert.equal(document.getText(), original);
        }
    });

    test("SSR preview applies edits to a quoted project source", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        // Exercise a project member whose path needs quoting in the manifest.
        const uri = vscode.Uri.joinPath(
            vscode.workspace.workspaceFolders![0].uri,
            "ssr'quoted.sail",
        );
        const source = "function quoted() -> int = 19283\n";
        const manifestUri = vscode.Uri.joinPath(
            vscode.workspace.workspaceFolders![0].uri,
            "sail.proj",
        );
        const manifest = await vscode.workspace.fs.readFile(manifestUri);
        await vscode.workspace.fs.writeFile(uri, Buffer.from(source));
        const document = await vscode.workspace.openTextDocument(uri);
        try {
            await vscode.workspace.fs.writeFile(
                manifestUri,
                Buffer.concat([manifest, Buffer.from('\nQuoted { files "ssr\'quoted.sail" }\n')]),
            );
            await eventually("quoted source was not loaded into the project", () =>
                JSON.stringify(api.test.projectGraph()).includes("quoted.sail"),
            );
            await vscode.window.showTextDocument(document, {
                selection: new vscode.Range(0, 0, 0, 0),
            });
            assert.equal(
                await vscode.commands.executeCommand(
                    "sail-lsp.structuralReplace",
                    "19283 ==>> 19284",
                ),
                1,
                "the preview must resolve the edited document through its parsed URI",
            );
            assert.equal(document.getText(), source, "a preview must not edit the source");
            assert.equal(
                await vscode.commands.executeCommand("sail-lsp.applyStructuralReplace"),
                true,
            );
            assert.equal(document.getText(), source.replace("19283", "(19284)"));
        } finally {
            await vscode.commands.executeCommand("sail-lsp.cancelStructuralReplace");
            await vscode.window.showTextDocument(document);
            await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor");
            await vscode.workspace.fs.writeFile(manifestUri, manifest);
            await vscode.workspace.fs.delete(uri);
            await eventually(
                "quoted source was not removed from the project",
                () => !JSON.stringify(api.test.projectGraph()).includes("quoted.sail"),
            );
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
            vscode.languages
                .getDiagnostics(unlinkedUri)
                .some(
                    (d) => (typeof d.code === "object" ? d.code.value : d.code) === "unlinked-file",
                ),
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
            await vscode.commands.executeCommand("leaveSnippet");
            await replace(original);
            await vscode.window.showTextDocument(document);
            await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor");
        }
    });

    test("editing commands find the matching brace, move items, join lines and complete Enter", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        await closeRestoredEditors();
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
        const select = async (position: vscode.Position) => {
            editor = await focusSource(document, position);
        };
        let editor: vscode.TextEditor;
        let observeEnter: vscode.Disposable | undefined;
        const enterChanges: unknown[] = [];
        try {
            await replace(source);
            editor = await vscode.window.showTextDocument(document);
            await select(document.positionAt(source.indexOf("{")));
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
            await select(new vscode.Position(0, 0));
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
            await select(new vscode.Position(opening, 0));
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
            await select(document.lineAt(doc).range.end);
            observeEnter = vscode.workspace.onDidChangeTextDocument((event) => {
                if (event.document === document)
                    enterChanges.push({ version: document.version, changes: event.contentChanges });
            });
            await vscode.commands.executeCommand("sail-lsp.onEnter");
            await eventually(
                () =>
                    `doc comment not continued: ${JSON.stringify({
                        text: document.getText(),
                        version: document.version,
                        selection: editor.selection,
                        changes: enterChanges,
                    })}`,
                () =>
                    document.lineAt(doc).text === "/// doc" &&
                    document.lineAt(doc + 1).text === "/// " &&
                    editor.selection.active.isEqual(new vscode.Position(doc + 1, 4)),
            );
            const before = document.lineCount;
            const plain = lineOf("function b() -> int = 2");
            await select(document.lineAt(plain).range.end);
            await vscode.commands.executeCommand("sail-lsp.onEnter");
            await eventually(
                () => `newline not typed: ${JSON.stringify(document.getText())}`,
                () => document.lineCount === before + 1 && document.lineAt(plain + 1).text === "",
            );
            await new Promise((resolve) => setTimeout(resolve, 300));
            assert.equal(document.lineAt(plain + 1).text, "", "plain code gained a continuation");
        } finally {
            observeEnter?.dispose();
            await vscode.commands.executeCommand("leaveSnippet");
            await replace(original);
            await vscode.window.showTextDocument(document);
            await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor");
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
            await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor");
        }
    });

    test("language configuration leaves a type variable's quote alone and continues block comments", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        await closeRestoredEditors();
        // A document first opened now: VS Code applies Enter rules registered
        // at activation to models created after it.
        const uri = vscode.Uri.joinPath(vscode.workspace.workspaceFolders![0].uri, "nested.sail");
        const document = await vscode.workspace.openTextDocument(uri);
        const original = document.getText();
        // The comment opener last: everything below an unterminated `/*` is
        // a comment, where no pair auto-closes.
        const source = "val x : forall \n/* note\n";
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
        const typeAt = async (position: vscode.Position, text: string) => {
            await focusSource(document, position);
            const version = document.version;
            await vscode.commands.executeCommand("type", { text });
            await eventually(
                () =>
                    `typing did not change the document: ${JSON.stringify({
                        typed: text,
                        before: version,
                        after: document.version,
                        text: document.getText(),
                        selection: vscode.window.activeTextEditor?.selection,
                        active: vscode.window.activeTextEditor?.document.uri.toString(),
                    })}`,
                () => document.version > version,
            );
        };
        try {
            await replace(source);
            // A pair that does auto-close first, so a bare quote below means
            // the configuration is in effect rather than absent.
            await typeAt(document.lineAt(0).range.end, "(");
            assert.equal(
                document.lineAt(0).text,
                "val x : forall ()",
                "the pairs are not in effect",
            );
            await typeAt(new vscode.Position(0, "val x : forall ".length), "'");
            assert.equal(
                document.lineAt(0).text,
                "val x : forall '()",
                "a closing quote was inserted",
            );
            await typeAt(document.lineAt(1).range.end, "\n");
            assert.equal(document.lineAt(2).text, " * ", "the block comment was not continued");
        } finally {
            await replace(original);
            await vscode.window.showTextDocument(document);
        }
    });

    test("hovering a val shows its implementation as an action link", async () => {
        const extension = vscode.extensions.getExtension<ExtensionApi>("Lunarwwww.sail-lsp")!;
        const api = await extension.activate();
        await eventually("server ready", () => api.test.status()?.quiescent === true);
        const uri = vscode.Uri.joinPath(vscode.workspace.workspaceFolders![0].uri, "sample.sail");
        const document = await vscode.workspace.openTextDocument(uri);
        const original = document.getText();
        const source = "val f : int -> int\nfunction f(x) = x\n";
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
        const rendered = async () => {
            const hovers = await vscode.commands.executeCommand<vscode.Hover[]>(
                "vscode.executeHoverProvider",
                uri,
                new vscode.Position(0, "val ".length),
            );
            return (hovers ?? [])
                .flatMap((hover) => hover.contents)
                .map((content) => (typeof content === "string" ? content : content.value))
                .join("\n");
        };
        try {
            await replace(source);
            await vscode.window.showTextDocument(document);
            let text = "";
            const deadline = Date.now() + 20_000;
            while (!text.includes("hoverRefCommandProxy") && Date.now() < deadline) {
                text = await rendered();
                if (!text.includes("hoverRefCommandProxy"))
                    await new Promise((resolve) => setTimeout(resolve, 50));
            }
            assert.match(
                text,
                /\[1 implementation\]\(command:sail-lsp\.hoverRefCommandProxy\?0 'Go to implementations'\)/,
                text,
            );
            // The link's command runs the stored `showReferences`.
            await vscode.commands.executeCommand("sail-lsp.hoverRefCommandProxy", 0);
            await vscode.commands.executeCommand("closeReferenceSearch");
        } finally {
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
