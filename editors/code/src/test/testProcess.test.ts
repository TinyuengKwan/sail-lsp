import * as assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { runTestProcess } from "../testProcess";

async function until(predicate: () => boolean): Promise<void> {
    const deadline = Date.now() + 10_000;
    while (Date.now() < deadline) {
        if (predicate()) return;
        await new Promise((resolve) => setTimeout(resolve, 25));
    }
    assert.fail("compiler process did not reach the expected state");
}

function running(pid: number): boolean {
    try {
        process.kill(pid, 0);
        if (process.platform === "linux") {
            // An orphan can remain as a zombie until the container's init
            // reaps it; it is dead and cannot retain the output pipes.
            const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
            return !/^Z\b/.test(stat.slice(stat.lastIndexOf(")") + 2));
        }
        return true;
    } catch {
        return false;
    }
}

test("interpreter process output determines the verdict and startup errors settle", async () => {
    const command = {
        program: process.execPath,
        args: ["-e", "console.log('Result = ()')"],
        cwd: tmpdir(),
        environment: {},
    };
    assert.deepEqual(await runTestProcess(command, new AbortController().signal, () => {}), {
        passed: true,
    });
    const missing = await runTestProcess(
        { ...command, program: join(tmpdir(), "sail-lsp-compiler-does-not-exist") },
        new AbortController().signal,
        () => {},
    );
    assert.equal(missing.passed, false);
    assert.match(missing.message!, /Could not start/);
});

test("an already cancelled test starts no compiler", async () => {
    const controller = new AbortController();
    controller.abort();
    assert.deepEqual(
        await runTestProcess(
            { program: "must-not-be-started", args: [], cwd: tmpdir(), environment: {} },
            controller.signal,
            () => {},
        ),
        { passed: false, message: "cancelled" },
    );
});

test("cancelling a test kills descendants holding its output pipes and settles", async () => {
    const directory = mkdtempSync(join(tmpdir(), "sail-lsp-test-process-"));
    const pidFile = join(directory, "pids.json");
    const controller = new AbortController();
    let pids: { parent: number; descendant: number } | undefined;
    let timer: NodeJS.Timeout | undefined;
    const script = `
        const {spawn} = require('node:child_process');
        const {writeFileSync} = require('node:fs');
        const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], {stdio:'inherit'});
        writeFileSync(${JSON.stringify(pidFile)}, JSON.stringify({parent:process.pid, descendant:child.pid}));
        setInterval(() => {}, 1000);
    `;
    const result = runTestProcess(
        { program: process.execPath, args: ["-e", script], cwd: directory, environment: {} },
        controller.signal,
        () => {},
    );
    try {
        await until(() => {
            try {
                pids = JSON.parse(readFileSync(pidFile, "utf8"));
                return true;
            } catch {
                return false;
            }
        });
        controller.abort();
        const timeout = new Promise<never>((_, reject) => {
            timer = setTimeout(
                () => reject(new Error("cancelled test retained its pipes")),
                10_000,
            );
        });
        assert.deepEqual(await Promise.race([result, timeout]), {
            passed: false,
            message: "cancelled",
        });
        await until(() => !running(pids!.parent) && !running(pids!.descendant));
    } finally {
        clearTimeout(timer);
        controller.abort();
        // Also clean up when the implementation regresses and the assertion fails.
        for (const pid of pids ? [pids.descendant, pids.parent] : []) {
            if (running(pid)) process.kill(pid, "SIGKILL");
        }
        await result;
        rmSync(directory, { recursive: true, force: true });
    }
});
