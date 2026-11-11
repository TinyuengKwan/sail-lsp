// The `$[test]` functions of open files in VS Code's Test Explorer —
// rust-analyzer's `test_explorer.ts`, fed by `experimental/runnables`
// rather than by a `discoverTest` request: upstream Sail has no test
// runner for the server to drive, so each test is an interpreter run the
// extension starts and reads the verdict of (`tasks.ts`'s `testOutcome`).
//
// Behind `sail-lsp.testExplorer`, off by default like rust-analyzer's.

import * as vscode from "vscode";
import type { Runnable, Runnables } from "./runnables";
import { isTestRunnable } from "./tasks";

export type TestExplorer = {
    /** The ids of the listed tests, for the tests. */
    tests: () => string[];
    /** Runs these tests (all when empty) as the run profile would, and
     * answers each one's verdict. */
    run: (ids: string[]) => Promise<Record<string, "passed" | "failed" | "errored">>;
};

export function registerTestExplorer(
    context: vscode.ExtensionContext,
    runnables: Runnables,
): TestExplorer {
    let controller: vscode.TestController | undefined;
    let runHandler:
        | ((request: vscode.TestRunRequest, token: vscode.CancellationToken) => Promise<void>)
        | undefined;
    const subscriptions: vscode.Disposable[] = [];
    const entries = new Map<string, Runnable & { kind: "shell" }>();
    const timers = new Map<string, NodeJS.Timeout>();
    const verdicts = new Map<string, "passed" | "failed" | "errored">();

    const enabled = (): boolean =>
        vscode.workspace.getConfiguration("sail-lsp").get<boolean>("testExplorer", false);

    const refreshFile = async (uri: vscode.Uri): Promise<void> => {
        if (!controller) return;
        const current = controller;
        const found = (await runnables.list(uri)).filter(isTestRunnable);
        if (controller !== current) return;
        const key = uri.toString();
        for (const id of [...entries.keys()]) {
            if (entries.get(id)!.location!.uri === key) entries.delete(id);
        }
        if (found.length === 0) {
            current.items.delete(key);
            return;
        }
        let file = current.items.get(key);
        if (!file) {
            file = current.createTestItem(key, vscode.workspace.asRelativePath(uri), uri);
            current.items.add(file);
        }
        file.children.replace(
            found.map((runnable) => {
                entries.set(runnable.id, runnable);
                const test = current.createTestItem(
                    runnable.id,
                    runnable.label.replace(/^Run test /, ""),
                    uri,
                );
                const { start, end } = runnable.location.range;
                test.range = new vscode.Range(start.line, start.character, end.line, end.character);
                return test;
            }),
        );
    };
    const refreshAll = (): void => {
        for (const document of vscode.workspace.textDocuments) {
            if (document.languageId === "sail" && document.uri.scheme === "file") {
                void refreshFile(document.uri);
            }
        }
    };
    const schedule = (document: vscode.TextDocument): void => {
        if (!controller || document.languageId !== "sail" || document.uri.scheme !== "file") return;
        const key = document.uri.toString();
        clearTimeout(timers.get(key));
        timers.set(
            key,
            setTimeout(() => {
                timers.delete(key);
                void refreshFile(document.uri);
            }, 300),
        );
    };

    const dispose = (): void => {
        for (const timer of timers.values()) clearTimeout(timer);
        timers.clear();
        for (const subscription of subscriptions.splice(0)) subscription.dispose();
        controller?.dispose();
        controller = undefined;
        runHandler = undefined;
        entries.clear();
    };
    const create = (): void => {
        if (controller) return;
        const current = vscode.tests.createTestController("sail-lsp", "Sail");
        controller = current;
        runHandler = async (request, token) => {
            const run = current.createTestRun(request);
            const selected: vscode.TestItem[] = [];
            const collect = (item: vscode.TestItem): void => {
                if (request.exclude?.includes(item)) return;
                if (entries.has(item.id)) selected.push(item);
                item.children.forEach(collect);
            };
            (request.include ?? [...current.items].map(([, item]) => item)).forEach(collect);
            try {
                for (const item of selected) {
                    if (token.isCancellationRequested) {
                        run.skipped(item);
                        continue;
                    }
                    const entry = entries.get(item.id)!;
                    run.started(item);
                    try {
                        const fresh = await runnables.select(
                            vscode.Uri.parse(entry.location!.uri),
                            entry.id,
                            entry.fingerprint,
                        );
                        if (fresh.kind !== "shell") throw new Error("not a process entry");
                        const verdict = await runnables.runTest(fresh, token, (chunk) =>
                            run.appendOutput(chunk.replace(/\r?\n/g, "\r\n"), undefined, item),
                        );
                        verdicts.set(item.id, verdict.passed ? "passed" : "failed");
                        if (verdict.passed) run.passed(item);
                        else run.failed(item, new vscode.TestMessage(verdict.message ?? "failed"));
                    } catch (error) {
                        verdicts.set(item.id, "errored");
                        run.errored(
                            item,
                            new vscode.TestMessage(
                                error instanceof Error ? error.message : String(error),
                            ),
                        );
                    }
                }
            } finally {
                run.end();
            }
        };
        current.createRunProfile("Run Tests", vscode.TestRunProfileKind.Run, runHandler, true);
        current.refreshHandler = () => {
            current.items.replace([]);
            entries.clear();
            refreshAll();
        };
        subscriptions.push(
            vscode.workspace.onDidOpenTextDocument(schedule),
            vscode.workspace.onDidChangeTextDocument((event) => schedule(event.document)),
            vscode.workspace.onDidCloseTextDocument((document) => {
                const key = document.uri.toString();
                clearTimeout(timers.get(key));
                timers.delete(key);
                current.items.delete(key);
                for (const id of [...entries.keys()]) {
                    if (entries.get(id)!.location!.uri === key) entries.delete(id);
                }
            }),
            runnables.onDidChange(refreshAll),
        );
        refreshAll();
    };

    if (enabled()) create();
    context.subscriptions.push(
        vscode.workspace.onDidChangeConfiguration((event) => {
            if (!event.affectsConfiguration("sail-lsp.testExplorer")) return;
            if (enabled()) create();
            else dispose();
        }),
        { dispose },
    );
    return {
        tests: () => [...entries.keys()],
        run: async (ids) => {
            if (!controller || !runHandler)
                throw new Error("The Sail Test Explorer is not enabled");
            const items: vscode.TestItem[] = [];
            controller.items.forEach((file) =>
                file.children.forEach((test) => {
                    if (ids.includes(test.id)) items.push(test);
                }),
            );
            await runHandler(
                new vscode.TestRunRequest(ids.length ? items : undefined),
                new vscode.CancellationTokenSource().token,
            );
            return Object.fromEntries(verdicts);
        },
    };
}
