import * as vscode from "vscode";
import { spawn } from "node:child_process";
import type { LanguageClient } from "vscode-languageclient/node";
import { TaskInvocations } from "./taskInvocations";
import { scriptArguments, taskEnvironment } from "./taskRunner";
import {
    Runnable,
    SAIL_PROBLEM_MATCHERS,
    SAIL_TASK_SOURCE,
    invocationCommand,
    lensLine,
    taskDefinition,
    taskGroup,
    testOutcome,
} from "./tasks";

export type { Runnable } from "./tasks";

export type Runnables = {
    list: (uri: vscode.Uri) => Promise<Runnable[]>;
    refresh: () => void;
    activeTasks: () => number;
    /** Fires when discovery may have changed: settings, manifests, projects. */
    onDidChange: vscode.Event<void>;
    /** The entry as discovered now, checked for execution: still the same
     * entry, not disabled, the workspace trusted, its inputs saved. */
    select: (uri: vscode.Uri, id: string, fingerprint: string) => Promise<Runnable>;
    /** The VS Code task that runs a selected process entry. */
    task: (selected: Runnable & { kind: "shell" }) => vscode.Task;
    /** The tasks the task provider offers now; their invocations are kept
     * until the next offer replaces them. */
    provide: (entries: (Runnable & { kind: "shell" })[]) => vscode.Task[];
    /** Runs a selected interpreter entry here and reads its verdict, for
     * the Test Explorer; the task terminal cannot be read back. */
    runTest: (
        selected: Runnable & { kind: "shell" },
        token: vscode.CancellationToken,
        output: (chunk: string) => void,
    ) => Promise<{ passed: boolean; message?: string }>;
};

/** Discover with the loaded server model; execute only on a user command. */
export function registerRunnables(
    context: vscode.ExtensionContext,
    getClient: () => LanguageClient | undefined,
    extraEnv: () => Record<string, string>,
): Runnables {
    const changed = new vscode.EventEmitter<void>();
    const running = new Set<vscode.TaskExecution>();
    const invocations = new TaskInvocations();
    const pending = new Map<string, number>();
    const provided = new Set<string>();
    const prune = (): void =>
        invocations.prune(
            new Set([
                ...pending.keys(),
                ...provided,
                ...[...running].map(
                    (execution) => execution.task.definition.sailInvocation as string,
                ),
            ]),
        );
    const list = async (uri: vscode.Uri): Promise<Runnable[]> => {
        const client = getClient();
        if (!client) return [];
        if (!client.initializeResult?.capabilities.experimental?.runnables) return [];
        return client.sendRequest<Runnable[]>("experimental/runnables", {
            textDocument: { uri: uri.toString() },
        });
    };
    const refresh = (): void => changed.fire();
    const select = async (uri: vscode.Uri, id: string, fingerprint: string): Promise<Runnable> => {
        if (!fingerprint) throw new Error("A project task fingerprint is required");
        const selected = (await list(uri)).find((task) => task.id === id);
        if (selected?.fingerprint !== fingerprint) {
            throw new Error("The project task changed; select it again before running");
        }
        if (selected.disabled) throw new Error(selected.disabled);
        if (!vscode.workspace.isTrusted) {
            throw new Error("Trust this workspace before running Sail project commands");
        }
        const documents = new Set(
            selected.documents.map((uri) => vscode.Uri.parse(uri).toString()),
        );
        const dirty = vscode.workspace.textDocuments.find(
            (document) => document.isDirty && documents.has(document.uri.toString()),
        );
        if (dirty) throw new Error(`Save ${dirty.fileName} before running the project task`);
        return selected;
    };
    const task = (selected: Runnable & { kind: "shell" }): vscode.Task => {
        const invocation = invocations.prepare(invocationCommand(selected, extraEnv()));
        const scope =
            vscode.workspace.getWorkspaceFolder(vscode.Uri.parse(selected.project.uri)) ??
            vscode.TaskScope.Workspace;
        const result = new vscode.Task(
            {
                ...taskDefinition(selected),
                id: selected.id,
                project: selected.project.uri,
                sailInvocation: invocation.key,
            },
            scope,
            selected.label,
            SAIL_TASK_SOURCE,
            launcherExecution(context, invocation.file),
            SAIL_PROBLEM_MATCHERS,
        );
        const group = taskGroup(selected.args.kind);
        result.group =
            group === "test"
                ? vscode.TaskGroup.Test
                : group === "build"
                  ? vscode.TaskGroup.Build
                  : undefined;
        result.presentationOptions = {
            reveal: vscode.TaskRevealKind.Always,
            panel: vscode.TaskPanelKind.Dedicated,
            echo: false,
        };
        return result;
    };
    const provide = (entries: (Runnable & { kind: "shell" })[]): vscode.Task[] => {
        const tasks = entries.map(task);
        provided.clear();
        for (const prepared of tasks) provided.add(prepared.definition.sailInvocation as string);
        prune();
        return tasks;
    };
    /** Keeps a prepared task's invocation file until the task has started;
     * a task that has started is kept by `running`. */
    const hold = (prepared: vscode.Task): (() => void) => {
        const key = prepared.definition.sailInvocation as string;
        pending.set(key, (pending.get(key) ?? 0) + 1);
        return () => {
            const count = pending.get(key)! - 1;
            if (count) pending.set(key, count);
            else pending.delete(key);
            prune();
        };
    };
    const runTest = (
        selected: Runnable & { kind: "shell" },
        token: vscode.CancellationToken,
        output: (chunk: string) => void,
    ): Promise<{ passed: boolean; message?: string }> =>
        new Promise((resolve) => {
            const command = invocationCommand(selected, extraEnv());
            const { args, cleanup } = scriptArguments(command.args, command.script);
            let collected = "";
            let child: ReturnType<typeof spawn>;
            try {
                child = spawn(command.program, args, {
                    cwd: command.cwd,
                    env: taskEnvironment(process.env, command.environment),
                    stdio: ["ignore", "pipe", "pipe"],
                });
            } catch (error) {
                cleanup();
                resolve({ passed: false, message: String(error) });
                return;
            }
            const read = (chunk: Buffer): void => {
                const text = chunk.toString();
                collected += text;
                output(text);
            };
            child.stdout?.on("data", read);
            child.stderr?.on("data", read);
            const cancel = token.onCancellationRequested(() => child.kill());
            child.once("error", (error) => {
                cancel.dispose();
                cleanup();
                resolve({
                    passed: false,
                    message: `Could not start ${command.program}: ${error.message}`,
                });
            });
            child.once("close", (code) => {
                cancel.dispose();
                cleanup();
                resolve(
                    token.isCancellationRequested
                        ? { passed: false, message: "cancelled" }
                        : testOutcome(code, collected),
                );
            });
        });
    context.subscriptions.push(
        changed,
        vscode.workspace.onDidChangeConfiguration((event) => {
            if (event.affectsConfiguration("sail-lsp")) refresh();
        }),
        vscode.workspace.onDidChangeTextDocument((event) => {
            if (event.document.languageId === "sail-project") refresh();
        }),
        vscode.workspace.onDidSaveTextDocument((document) => {
            if (document.languageId === "sail-project") refresh();
        }),
        vscode.tasks.onDidStartTask((event) => {
            if (event.execution.task.source === SAIL_TASK_SOURCE) running.add(event.execution);
        }),
        vscode.tasks.onDidEndTask((event) => {
            running.delete(event.execution);
            prune();
        }),
        {
            dispose: () => {
                for (const execution of running) execution.terminate();
                invocations.dispose();
            },
        },
        vscode.commands.registerCommand(
            "sail-lsp.runProjectTask",
            async (source?: vscode.Uri | string, id?: string, fingerprint?: string) => {
                const uri =
                    typeof source === "string"
                        ? vscode.Uri.parse(source)
                        : (source ?? vscode.window.activeTextEditor?.document.uri);
                if (!uri) return undefined;
                if (id === undefined) {
                    const candidates = await list(uri);
                    if (candidates.length === 0) {
                        void vscode.window.showInformationMessage(
                            "No project tasks are available for this file; check the loaded project and server version.",
                        );
                        return undefined;
                    }
                    const item = await vscode.window.showQuickPick(
                        candidates.map((runnable) => ({
                            label: runnable.label,
                            description: runnable.disabled ?? runnable.kind,
                            detail: runnable.project.uri,
                            runnable,
                        })),
                        {
                            title: "Run Sail Project Task",
                            placeHolder: "Choose a project check, test or export task",
                        },
                    );
                    if (!item) return undefined;
                    id = item.runnable.id;
                    fingerprint = item.runnable.fingerprint;
                }
                const selected = await select(uri, id, fingerprint ?? "");
                const client = getClient();
                if (!client) throw new Error("The Sail server is not running");
                if (selected.kind === "sail-check") {
                    await client.sendNotification("sail-lsp/runFlycheck", selected.args);
                    return undefined;
                }
                const prepared = task(selected);
                const release = hold(prepared);
                try {
                    return await vscode.tasks.executeTask(prepared);
                } finally {
                    release();
                }
            },
        ),
        vscode.commands.registerCommand("sail-lsp.cancelProjectTasks", async () => {
            for (const execution of running) execution.terminate();
            await getClient()?.sendNotification("sail-lsp/cancelFlycheck", {});
        }),
        vscode.languages.registerCodeLensProvider(
            [
                { language: "sail", scheme: "file" },
                { language: "sail-project", scheme: "file" },
            ],
            {
                onDidChangeCodeLenses: changed.event,
                async provideCodeLenses(document, token) {
                    if (
                        !vscode.workspace
                            .getConfiguration("sail-lsp", document.uri)
                            .get("runnables.showCodeLens", true)
                    )
                        return [];
                    const tasks = await list(document.uri);
                    if (token.isCancellationRequested) return [];
                    return tasks
                        .filter((task) => !task.disabled)
                        .map(
                            (task) =>
                                new vscode.CodeLens(
                                    new vscode.Range(lensLine(task), 0, lensLine(task), 0),
                                    {
                                        title: task.location ? `▶ ${task.label}` : task.label,
                                        command: "sail-lsp.runProjectTask",
                                        arguments: [
                                            document.uri.toString(),
                                            task.id,
                                            task.fingerprint,
                                        ],
                                    },
                                ),
                        );
                },
            },
        ),
    );
    return {
        list,
        refresh,
        activeTasks: () => running.size,
        onDidChange: changed.event,
        select,
        task,
        provide,
        runTest,
    };
}

/**
 * The execution every Sail task uses: VS Code's own Node runtime starts the
 * bundled launcher, which reads the command from a private invocation file.
 *
 * Electron is a GUI executable on Windows: ConPTY can execute it without
 * reporting the process events used by Tasks. A console shell keeps those
 * events and the real exit status. Only fixed code and private path
 * variables enter the shell; user arguments/environment are read from JSON
 * afterwards.
 */
export function launcherExecution(
    context: vscode.ExtensionContext,
    invocationFile: string,
): vscode.ProcessExecution | vscode.ShellExecution {
    const launcher = vscode.Uri.file(context.asAbsolutePath("out/taskRunner.js")).toString();
    const invocationUri = vscode.Uri.file(invocationFile).toString();
    const bootstrap =
        "require(require('node:url').fileURLToPath(process.argv[1])).runTask(process.argv[2]);";
    return process.platform === "win32"
        ? new vscode.ShellExecution(
              `""%SAIL_LSP_TASK_RUNTIME%" -e "${bootstrap}" "%SAIL_LSP_TASK_LAUNCHER%" "%SAIL_LSP_TASK_INVOCATION%""`,
              {
                  executable: process.env.ComSpec ?? "cmd.exe",
                  shellArgs: ["/d", "/v:off", "/s", "/c"],
                  env: {
                      ELECTRON_RUN_AS_NODE: "1",
                      SAIL_LSP_TASK_RUNTIME: process.execPath,
                      SAIL_LSP_TASK_LAUNCHER: launcher,
                      SAIL_LSP_TASK_INVOCATION: invocationUri,
                  },
              },
          )
        : new vscode.ProcessExecution(
              process.execPath,
              ["-e", bootstrap, launcher, invocationUri],
              {
                  env: { ELECTRON_RUN_AS_NODE: "1" },
              },
          );
}
