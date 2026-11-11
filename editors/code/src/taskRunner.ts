import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { constants, tmpdir } from "node:os";
import { join } from "node:path";

const launcherVariables = new Set([
    "ELECTRON_RUN_AS_NODE",
    "SAIL_LSP_TASK_RUNTIME",
    "SAIL_LSP_TASK_LAUNCHER",
    "SAIL_LSP_TASK_INVOCATION",
]);

export function taskEnvironment(
    inherited: NodeJS.ProcessEnv,
    overrides: Record<string, string>,
    windows = process.platform === "win32",
): NodeJS.ProcessEnv {
    const result: NodeJS.ProcessEnv = Object.create(null);
    for (const [key, value] of Object.entries(inherited)) {
        if (!launcherVariables.has(windows ? key.toUpperCase() : key)) result[key] = value;
    }
    for (const [key, value] of Object.entries(overrides)) {
        if (windows) {
            for (const existing of Object.keys(result)) {
                if (existing.toUpperCase() === key.toUpperCase()) delete result[existing];
            }
        }
        result[key] = value;
    }
    return result;
}

/** An interpreter script lives in a file for as long as the process that
 * reads it runs; `--is` takes a path and the interpreter reads it only once
 * the project has type-checked. */
export function scriptArguments(
    args: string[],
    script: string | undefined,
): { args: string[]; cleanup: () => void } {
    if (script === undefined) return { args, cleanup: () => {} };
    const directory = mkdtempSync(join(tmpdir(), "sail-lsp-script-"));
    const file = join(directory, "run.isail");
    writeFileSync(file, script, { mode: 0o600 });
    return {
        args: [...args, file],
        cleanup: () => rmSync(directory, { recursive: true, force: true }),
    };
}

/** Invoked by VS Code's bundled Node runtime. Arguments and environment are
 * read after task variable resolution, then passed directly to the child. */
export function runTask(uri: string): void {
    let spec: {
        version: number;
        command: {
            program: string;
            args: string[];
            cwd: string;
            environment: Record<string, string>;
            script?: string;
        };
    };
    try {
        spec = JSON.parse(readFileSync(fileURLToPath(uri), "utf8"));
        if (
            spec.version !== 1 ||
            typeof spec.command?.program !== "string" ||
            !Array.isArray(spec.command.args) ||
            !spec.command.args.every((arg) => typeof arg === "string") ||
            typeof spec.command.cwd !== "string" ||
            !spec.command.environment ||
            typeof spec.command.environment !== "object" ||
            Array.isArray(spec.command.environment) ||
            !Object.values(spec.command.environment).every((value) => typeof value === "string") ||
            (spec.command.script !== undefined && typeof spec.command.script !== "string")
        ) {
            throw new Error("invalid invocation");
        }
    } catch {
        console.error(
            "Sail project task invocation is unavailable or invalid; select the task again.",
        );
        process.exitCode = 1;
        return;
    }
    const environment = taskEnvironment(process.env, spec.command.environment);
    try {
        const { args, cleanup } = scriptArguments(spec.command.args, spec.command.script);
        const child = spawn(spec.command.program, args, {
            cwd: spec.command.cwd,
            env: environment,
            stdio: "inherit",
        });
        child.once("error", (error) => {
            cleanup();
            console.error(`Could not start Sail project task: ${error.message}`);
            process.exitCode = 1;
        });
        child.once("exit", (code, signal) => {
            cleanup();
            if (code !== null) {
                process.exitCode = code;
            } else {
                console.error(`Sail project task terminated by ${signal ?? "an unknown signal"}`);
                process.exitCode = 128 + (signal ? (constants.signals[signal] ?? 1) : 1);
            }
        });
    } catch (error) {
        console.error(`Could not start Sail project task: ${String(error)}`);
        process.exitCode = 1;
    }
}
