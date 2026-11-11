// What the server discovers and what VS Code's task system runs, and the
// conversion between them — rust-analyzer's `tasks.ts`, less the `vscode`
// import so the conversion is testable on its own.

type Document = { uri: string };
type Position = { line: number; character: number };

export type ProcessArgs = {
    kind: "test" | "export" | "run";
    program: string;
    args: string[];
    cwd: string;
    environment: Record<string, string>;
    /** Interpreter script contents; `args` end with the flag that names the
     * file it is stored in, and the launcher appends that file's path. */
    script?: string;
};

export type Runnable = {
    id: string;
    label: string;
    project: Document;
    fingerprint: string;
    disabled?: string;
    documents: string[];
    /** Where the entry belongs in the source; a project task has none. */
    location?: { uri: string; range: { start: Position; end: Position } };
} & (
    | { kind: "sail-check"; args: { textDocument: Document } }
    | { kind: "shell"; args: ProcessArgs }
);

/** The `type` of a Sail task in `tasks.json`. */
export const SAIL_TASK_TYPE = "sail";
export const SAIL_TASK_SOURCE = "sail-lsp";
/** The problem matchers `package.json` contributes, applied to every Sail task. */
export const SAIL_PROBLEM_MATCHERS = ["$sail", "$sail-warning"];

/** A `tasks.json` entry of `type: "sail"`, as `package.json` declares it. */
export type SailTaskDefinition = {
    type: typeof SAIL_TASK_TYPE;
    command: string;
    args?: string[];
    cwd?: string;
    env?: Record<string, string>;
    script?: string;
};

export function isSailTaskDefinition(definition: {
    type: string;
    [key: string]: unknown;
}): definition is SailTaskDefinition {
    return (
        definition.type === SAIL_TASK_TYPE &&
        typeof definition.command === "string" &&
        (definition.args === undefined ||
            (Array.isArray(definition.args) &&
                definition.args.every((arg) => typeof arg === "string"))) &&
        (definition.cwd === undefined || typeof definition.cwd === "string") &&
        (definition.env === undefined ||
            (typeof definition.env === "object" &&
                definition.env !== null &&
                Object.values(definition.env).every((value) => typeof value === "string"))) &&
        (definition.script === undefined || typeof definition.script === "string")
    );
}

/** The task definition of a process runnable. The environment is not part
 * of it: the task's identity is what it runs, and declared environment
 * values are private to the launcher's invocation file. */
export function taskDefinition(runnable: Runnable & { kind: "shell" }): SailTaskDefinition {
    const { program, args, cwd, script } = runnable.args;
    return { type: SAIL_TASK_TYPE, command: program, args, cwd, ...(script ? { script } : {}) };
}

/** The process the launcher starts for a runnable, with the editor's extra
 * environment under the task's own. */
export function invocationCommand(
    runnable: Runnable & { kind: "shell" },
    extraEnv: Record<string, string>,
): ProcessArgs {
    return { ...runnable.args, environment: { ...extraEnv, ...runnable.args.environment } };
}

/** `tasks.json` entries name no runnable; the launcher runs them as written,
 * in the folder given or the workspace folder. */
export function invocationFromDefinition(
    definition: SailTaskDefinition,
    workspaceFolder: string,
): Omit<ProcessArgs, "kind"> {
    return {
        program: definition.command,
        args: definition.args ?? [],
        cwd: definition.cwd ?? workspaceFolder,
        environment: definition.env ?? {},
        ...(definition.script ? { script: definition.script } : {}),
    };
}

/** Which task group a runnable belongs to: tests, builds, or none for `run`. */
export function taskGroup(kind: ProcessArgs["kind"]): "test" | "build" | undefined {
    return kind === "test" ? "test" : kind === "export" ? "build" : undefined;
}

/** The line a run lens sits on: the entry's own, or the top of the file. */
export function lensLine(runnable: Runnable): number {
    return runnable.location?.range.start.line ?? 0;
}

/** A `$[test]` function the Test Explorer lists: placed in the source and
 * run as a test. `main` is placed too, but is run, not tested. */
export function isTestRunnable(
    runnable: Runnable,
): runnable is Runnable & { kind: "shell"; location: NonNullable<Runnable["location"]> } {
    return (
        runnable.kind === "shell" &&
        runnable.args.kind === "test" &&
        runnable.location !== undefined
    );
}

/**
 * Whether an interpreter run passed. Upstream Sail has no test runner: the
 * interpreter exits 0 whatever happened, prints `Result = ()` when the
 * expression evaluated, and `Error: <reason>` when an assertion failed, an
 * exception escaped or the function was not found (then a type error).
 */
export function testOutcome(
    exitCode: number | null,
    output: string,
): { passed: boolean; message?: string } {
    const lines = output.split(/\r?\n/);
    const error = lines.find((line) => /^Error: /.test(line));
    if (error) return { passed: false, message: error };
    const typeError = lines.findIndex((line) => /^(Type|Syntax|Lexical) error:?/.test(line));
    if (typeError !== -1) {
        return { passed: false, message: lines.slice(typeError).join("\n").trim() };
    }
    if (exitCode !== 0) return { passed: false, message: `exit code ${exitCode}` };
    if (!lines.some((line) => /^Result = /.test(line))) {
        return { passed: false, message: "the interpreter produced no result" };
    }
    return { passed: true };
}
