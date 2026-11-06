import * as vscode from "vscode";
import type { LanguageClient } from "vscode-languageclient/node";
import { TaskInvocations } from "./taskInvocations";

type Document = { uri: string };
type ProcessArgs = {
    kind: "test" | "export";
    program: string;
    args: string[];
    cwd: string;
    environment: Record<string, string>;
};
export type Runnable = {
    id: string;
    label: string;
    project: Document;
    fingerprint: string;
    disabled?: string;
    documents: string[];
} & (
    | { kind: "sail-check"; args: { textDocument: Document } }
    | { kind: "shell"; args: ProcessArgs }
);

/** Discover with the loaded server model; execute only on a user command. */
export function registerRunnables(
    context: vscode.ExtensionContext,
    getClient: () => LanguageClient | undefined,
    extraEnv: () => Record<string, string>,
): {
    list: (uri: vscode.Uri) => Promise<Runnable[]>;
    refresh: () => void;
    activeTasks: () => number;
} {
    const changed = new vscode.EventEmitter<void>();
    const running = new Set<vscode.TaskExecution>();
    const invocations = new TaskInvocations();
    const pending = new Map<string, number>();
    const prune = (): void =>
        invocations.prune(
            new Set([
                ...pending.keys(),
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
            if (event.execution.task.source === "sail-lsp") running.add(event.execution);
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
                let selected: Runnable | undefined;
                const candidates = await list(uri);
                if (candidates.length === 0 && id === undefined) {
                    void vscode.window.showInformationMessage(
                        "No project tasks are available for this file; check the loaded project and server version.",
                    );
                    return undefined;
                }
                if (id !== undefined) {
                    if (!fingerprint) throw new Error("A project task fingerprint is required");
                    selected = candidates.find((task) => task.id === id);
                    if (selected?.fingerprint !== fingerprint) {
                        throw new Error("The project task changed; select it again before running");
                    }
                } else {
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
                    const fresh = await list(uri);
                    selected = fresh.find((task) => task.id === item.runnable.id);
                    if (selected?.fingerprint !== item.runnable.fingerprint) {
                        throw new Error("The project task changed; select it again before running");
                    }
                }
                if (!selected) throw new Error("The project task is no longer available");
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
                if (dirty)
                    throw new Error(`Save ${dirty.fileName} before running the project task`);
                const client = getClient();
                if (!client) throw new Error("The Sail server is not running");
                if (selected.kind === "sail-check") {
                    await client.sendNotification("sail-lsp/runFlycheck", selected.args);
                    return undefined;
                }
                const command = selected.args;
                const invocation = invocations.prepare({
                    ...command,
                    environment: { ...extraEnv(), ...command.environment },
                });
                pending.set(invocation.key, (pending.get(invocation.key) ?? 0) + 1);
                const launcher = vscode.Uri.file(
                    context.asAbsolutePath("out/taskRunner.js"),
                ).toString();
                const invocationUri = vscode.Uri.file(invocation.file).toString();
                const bootstrap =
                    "require(require('node:url').fileURLToPath(process.argv[1])).runTask(process.argv[2]);";
                // Electron is a GUI executable on Windows: ConPTY can execute
                // it without reporting the process events used by Tasks. A
                // console shell keeps those events and the real exit status.
                // Only fixed code and private path variables enter the shell;
                // user arguments/environment are read from JSON afterwards.
                const execution =
                    process.platform === "win32"
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
                              { env: { ELECTRON_RUN_AS_NODE: "1" } },
                          );
                const scope =
                    vscode.workspace.getWorkspaceFolder(vscode.Uri.parse(selected.project.uri)) ??
                    vscode.TaskScope.Workspace;
                const task = new vscode.Task(
                    {
                        type: "sail",
                        id: selected.id,
                        project: selected.project.uri,
                        sailInvocation: invocation.key,
                    },
                    scope,
                    selected.label,
                    "sail-lsp",
                    execution,
                    [],
                );
                task.group =
                    command.kind === "test" ? vscode.TaskGroup.Test : vscode.TaskGroup.Build;
                task.presentationOptions = {
                    reveal: vscode.TaskRevealKind.Always,
                    panel: vscode.TaskPanelKind.Dedicated,
                    echo: false,
                };
                try {
                    return await vscode.tasks.executeTask(task);
                } finally {
                    const count = pending.get(invocation.key)! - 1;
                    if (count) pending.set(invocation.key, count);
                    else pending.delete(invocation.key);
                    prune();
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
                                new vscode.CodeLens(new vscode.Range(0, 0, 0, 0), {
                                    title: task.label,
                                    command: "sail-lsp.runProjectTask",
                                    arguments: [document.uri.toString(), task.id, task.fingerprint],
                                }),
                        );
                },
            },
        ),
    );
    return { list, refresh, activeTasks: () => running.size };
}
