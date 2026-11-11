// Sail tasks in VS Code's task system — rust-analyzer's `tasks.ts`.
//
// `provideTasks` offers what the server discovers (project tasks and the
// `$[test]`/`main` entries of open files) as `type: "sail"` tasks;
// `resolveTask` gives the entries a user writes into `tasks.json` their
// execution, and re-discovers one of ours a `tasks.json` customization
// names, so it runs only if unchanged.

import * as vscode from "vscode";
import { TaskInvocations } from "./taskInvocations";
import { launcherExecution, Runnable, Runnables } from "./runnables";
import {
    SAIL_PROBLEM_MATCHERS,
    SAIL_TASK_SOURCE,
    SAIL_TASK_TYPE,
    invocationFromDefinition,
    isSailTaskDefinition,
} from "./tasks";

export function registerTaskProvider(
    context: vscode.ExtensionContext,
    runnables: Runnables,
    projectManifests: () => string[],
): vscode.Disposable {
    const invocations = new TaskInvocations();
    const provideTasks = async (): Promise<vscode.Task[]> => {
        if (!vscode.workspace.workspaceFolders) return [];
        const sources = new Set<string>(projectManifests());
        for (const document of vscode.workspace.textDocuments) {
            if (document.languageId === "sail" && document.uri.scheme === "file") {
                sources.add(document.uri.toString());
            }
        }
        const seen = new Set<string>();
        const entries: (Runnable & { kind: "shell" })[] = [];
        for (const source of sources) {
            for (const runnable of await runnables.list(vscode.Uri.parse(source))) {
                if (runnable.kind !== "shell" || runnable.disabled || seen.has(runnable.id))
                    continue;
                seen.add(runnable.id);
                entries.push(runnable);
            }
        }
        // A provided task is run as provided — VS Code resolves only the
        // tasks a user writes — so each carries its launcher execution.
        return runnables.provide(entries);
    };
    const resolveTask = async (task: vscode.Task): Promise<vscode.Task | undefined> => {
        const definition = task.definition;
        if (!isSailTaskDefinition(definition)) return undefined;
        const matchers = task.problemMatchers.length ? task.problemMatchers : SAIL_PROBLEM_MATCHERS;
        // One of ours: discover it again, so it runs with the environment
        // the definition deliberately leaves out, and only if unchanged.
        const { id, project } = task.definition as { id?: unknown; project?: unknown };
        if (typeof id === "string" && typeof project === "string") {
            const fresh = (await runnables.list(vscode.Uri.parse(project))).find(
                (runnable) => runnable.id === id,
            );
            if (!fresh || fresh.kind !== "shell") return undefined;
            const selected = await runnables.select(
                vscode.Uri.parse(project),
                fresh.id,
                fresh.fingerprint,
            );
            if (selected.kind !== "shell") return undefined;
            return runnables.task(selected);
        }
        const folder =
            task.scope && typeof task.scope === "object"
                ? task.scope.uri.fsPath
                : vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
        if (!folder) return undefined;
        const command = invocationFromDefinition(definition, folder);
        // A written task runs as written, through the process API that
        // resolves `${workspaceFolder}` and friends. An interpreter script
        // needs the launcher to store it, which reads the task as text.
        const execution = command.script
            ? launcherExecution(context, invocations.prepare(command).file)
            : new vscode.ProcessExecution(command.program, command.args, {
                  cwd: command.cwd,
                  env: command.environment,
              });
        return new vscode.Task(
            definition,
            task.scope ?? vscode.TaskScope.Workspace,
            task.name,
            SAIL_TASK_SOURCE,
            execution,
            matchers,
        );
    };
    return vscode.Disposable.from(
        vscode.tasks.registerTaskProvider(SAIL_TASK_TYPE, { provideTasks, resolveTask }),
        { dispose: () => invocations.dispose() },
    );
}
