import { test } from "node:test";
import * as assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { TaskInvocations } from "../taskInvocations";
import { taskEnvironment } from "../taskRunner";

test("opaque invocation files preserve literals, retain active tasks and reclaim old payloads", () => {
    const store = new TaskInvocations();
    const command = {
        program: "chosen",
        args: ["${workspaceFolder}", "$(literal)", "中文"],
        environment: { API_TOKEN: "private" },
    };
    const first = store.prepare(command);
    const root = path.dirname(first.file);
    try {
        assert.deepEqual(JSON.parse(fs.readFileSync(first.file, "utf8")).command, command);
        assert.equal(store.prepare(command).file, first.file);
        if (process.platform !== "win32") assert.equal(fs.statSync(first.file).mode & 0o077, 0);
        const active = new Set([first.key]);
        for (let index = 0; index < 80; index++) {
            store.prepare({ ...command, args: [String(index)] });
            store.prune(active);
        }
        assert.equal(fs.readdirSync(root).length, 64);
        assert.ok(fs.existsSync(first.file), "an active task's payload survives reclamation");
        store.prepare({ ...command, args: ["last"] });
        store.prune(new Set());
        assert.ok(!fs.existsSync(first.file), "inactive old payloads are reclaimed");
    } finally {
        store.dispose();
    }
    assert.ok(!fs.existsSync(root));
});

test("task environment removes the launcher flag and preserves literal and special-name values", () => {
    const overrides = JSON.parse('{"LITERAL":"${workspaceFolder}","__proto__":"value"}');
    const environment = taskEnvironment(
        {
            ELECTRON_RUN_AS_NODE: "1",
            SAIL_LSP_TASK_RUNTIME: "private runtime",
            SAIL_LSP_TASK_LAUNCHER: "private launcher",
            SAIL_LSP_TASK_INVOCATION: "private invocation",
            KEEP: "yes",
        },
        overrides,
        false,
    );
    assert.equal(environment.ELECTRON_RUN_AS_NODE, undefined);
    assert.equal(environment.SAIL_LSP_TASK_RUNTIME, undefined);
    assert.equal(environment.SAIL_LSP_TASK_LAUNCHER, undefined);
    assert.equal(environment.SAIL_LSP_TASK_INVOCATION, undefined);
    assert.equal(environment.LITERAL, "${workspaceFolder}");
    assert.equal(environment.__proto__, "value");
    assert.equal(environment.KEEP, "yes");
});

test("Windows environment overrides are case insensitive", () => {
    const environment = taskEnvironment(
        { PATH: "old", electron_run_as_node: "1", sail_lsp_task_runtime: "private" },
        { Path: "chosen" },
        true,
    );
    assert.deepEqual(Object.keys(environment), ["Path"]);
    assert.equal(environment.Path, "chosen");
});
