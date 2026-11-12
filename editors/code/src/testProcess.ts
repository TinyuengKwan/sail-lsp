import { spawn, type ChildProcess } from "node:child_process";
import { join } from "node:path";
import { scriptArguments, taskEnvironment } from "./taskRunner";
import { testOutcome, type ProcessArgs } from "./tasks";

/** Cancel the owned compiler tree, including children holding its output pipes. */
function terminate(child: ChildProcess): Promise<void> {
    if (child.pid === undefined) return Promise.resolve();
    if (process.platform !== "win32") {
        try {
            process.kill(-child.pid, "SIGKILL");
        } catch {
            // A child can exit before cancellation, or before creating its group.
            child.kill("SIGKILL");
        }
        return Promise.resolve();
    }
    return new Promise((resolve) => {
        const taskkill = spawn(
            join(process.env.SystemRoot ?? "C:\\Windows", "System32", "taskkill.exe"),
            ["/PID", String(child.pid), "/T", "/F"],
            { stdio: "ignore", windowsHide: true },
        );
        taskkill.once("error", () => {
            child.kill("SIGKILL");
            resolve();
        });
        taskkill.once("close", () => resolve());
    });
}

/** Interpreter runs use their own process group rather than the task terminal. */
export async function runTestProcess(
    command: Omit<ProcessArgs, "kind">,
    signal: AbortSignal,
    output: (chunk: string) => void,
): Promise<{ passed: boolean; message?: string }> {
    if (signal.aborted) return { passed: false, message: "cancelled" };
    const { args, cleanup } = scriptArguments(command.args, command.script);
    try {
        return await new Promise((resolve) => {
            const child = spawn(command.program, args, {
                cwd: command.cwd,
                env: taskEnvironment(process.env, command.environment),
                stdio: ["ignore", "pipe", "pipe"],
                detached: process.platform !== "win32",
                windowsHide: true,
            });
            let collected = "";
            let error: Error | undefined;
            let termination: Promise<void> | undefined;
            const cancel = (): void => {
                termination ??= terminate(child);
            };
            const read = (chunk: string): void => {
                collected += chunk;
                output(chunk);
            };
            child.stdout?.setEncoding("utf8").on("data", read);
            child.stderr?.setEncoding("utf8").on("data", read);
            signal.addEventListener("abort", cancel, { once: true });
            if (signal.aborted) cancel();
            child.once("error", (failure) => {
                error = failure;
            });
            child.once("close", async (code) => {
                await termination;
                signal.removeEventListener("abort", cancel);
                resolve(
                    signal.aborted
                        ? { passed: false, message: "cancelled" }
                        : error
                          ? {
                                passed: false,
                                message: `Could not start ${command.program}: ${error.message}`,
                            }
                          : testOutcome(code, collected),
                );
            });
        });
    } finally {
        cleanup();
    }
}
