import { test } from "node:test";
import * as assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import {
    Runnable,
    invocationCommand,
    invocationFromDefinition,
    isSailTaskDefinition,
    isTestRunnable,
    lensLine,
    taskDefinition,
    taskGroup,
    testOutcome,
} from "../tasks";

const project = { uri: "file:///work/model/model.sail_project" };
const testRunnable: Runnable = {
    id: "file:///work/model/model.sail_project#run-test_double",
    label: "Run test test_double",
    project,
    fingerprint: "f1",
    documents: ["file:///work/model/t.sail"],
    location: {
        uri: "file:///work/model/t.sail",
        range: { start: { line: 7, character: 0 }, end: { line: 10, character: 1 } },
    },
    kind: "shell",
    args: {
        kind: "test",
        program: "/tool chain/sail",
        args: ["--project", "model.sail_project", "Root", "--is"],
        cwd: "/work/model",
        environment: { SAIL_DIR: "/opt/sail" },
        script: "test_double()\n:run\n:quit\n",
    },
};
const exportRunnable: Runnable = {
    id: "file:///work/model/model.sail_project#task-0",
    label: "Export C (model.sail_project [Root])",
    project,
    fingerprint: "f2",
    documents: [],
    kind: "shell",
    args: {
        kind: "export",
        program: "sail",
        args: ["-c", "--project", "model.sail_project", "Root", "-o", "out/model"],
        cwd: "/work/model",
        environment: {},
    },
};
const checkRunnable: Runnable = {
    id: "file:///work/model/model.sail_project#check",
    label: "Check model.sail_project [Root]",
    project,
    fingerprint: "f3",
    documents: [],
    kind: "sail-check",
    args: { textDocument: project },
};

test("a runnable becomes a sail task: what it runs is its identity, its environment is not", () => {
    assert.deepEqual(taskDefinition(testRunnable), {
        type: "sail",
        command: "/tool chain/sail",
        args: ["--project", "model.sail_project", "Root", "--is"],
        cwd: "/work/model",
        script: "test_double()\n:run\n:quit\n",
    });
    assert.ok(!("env" in taskDefinition(testRunnable)));
    assert.ok(!("script" in taskDefinition(exportRunnable)));
    assert.equal(taskGroup("test"), "test");
    assert.equal(taskGroup("export"), "build");
    assert.equal(taskGroup("run"), undefined);
    // The editor's extra environment sits under the task's own.
    assert.deepEqual(
        invocationCommand(testRunnable, { SAIL_DIR: "/editor/sail", EXTRA: "1" }).environment,
        { EXTRA: "1", SAIL_DIR: "/opt/sail" },
    );
    assert.equal(lensLine(testRunnable), 7, "a placed entry's lens sits on its line");
    assert.equal(lensLine(exportRunnable), 0, "a project task's lens sits at the top");
    assert.ok(isTestRunnable(testRunnable));
    assert.ok(!isTestRunnable(exportRunnable));
    assert.ok(!isTestRunnable(checkRunnable));
    assert.ok(
        !isTestRunnable({ ...testRunnable, args: { ...testRunnable.args, kind: "run" } }),
        "main is run, not tested",
    );
});

test("tasks.json entries are validated and run as written", () => {
    assert.ok(isSailTaskDefinition({ type: "sail", command: "sail" }));
    assert.ok(
        isSailTaskDefinition({
            type: "sail",
            command: "sail",
            args: ["-c"],
            cwd: ".",
            env: { A: "1" },
            script: "main()\n:run\n:quit\n",
        }),
    );
    assert.ok(!isSailTaskDefinition({ type: "sail" }));
    assert.ok(!isSailTaskDefinition({ type: "sail", command: "sail", args: [1] }));
    assert.ok(!isSailTaskDefinition({ type: "sail", command: "sail", env: { A: 1 } }));
    assert.ok(!isSailTaskDefinition({ type: "shell", command: "sail" }));
    assert.deepEqual(
        invocationFromDefinition({ type: "sail", command: "sail", args: ["-c"] }, "/work"),
        { program: "sail", args: ["-c"], cwd: "/work", environment: {} },
    );
    assert.deepEqual(
        invocationFromDefinition(
            { type: "sail", command: "sail", cwd: "/elsewhere", script: "x" },
            "/work",
        ),
        { program: "sail", args: [], cwd: "/elsewhere", environment: {}, script: "x" },
    );
});

// `sail t.sail --is run.isail`, Sail 0.20.2. The interpreter exits 0 in every
// case, so the verdict is read from what it printed.
test("an interpreter run's verdict is read from its output", () => {
    assert.deepEqual(testOutcome(0, "test_double()\nok test_double\nResult = ()\n"), {
        passed: true,
    });
    assert.deepEqual(testOutcome(0, "test_fail()\nError: Assertion failed: double should fail\n"), {
        passed: false,
        message: "Error: Assertion failed: double should fail",
    });
    assert.deepEqual(testOutcome(0, "test_throw()\nError: Uncaught exception: E(())\n"), {
        passed: false,
        message: "Error: Uncaught exception: E(())",
    });
    const missing =
        "Type error:\nREPL:2.0-4:\n2 |nope()\n  |^--^\n  | Function nope not found\nCommand :run is not a valid command in this mode.\n";
    assert.equal(testOutcome(0, missing).passed, false);
    assert.match(testOutcome(0, missing).message!, /Function nope not found/);
    assert.deepEqual(testOutcome(1, ""), { passed: false, message: "exit code 1" });
    assert.deepEqual(testOutcome(0, "test_double()\n"), {
        passed: false,
        message: "the interpreter produced no result",
    });
});

type Pattern = {
    regexp: string;
    file?: number;
    line?: number;
    column?: number;
    endLine?: number;
    endColumn?: number;
    message?: number;
};
type Matcher = { name: string; pattern: Pattern | Pattern[] };

function matchers(): Record<string, Pattern[]> {
    const manifest = JSON.parse(
        fs.readFileSync(path.join(__dirname, "../../../package.json"), "utf8"),
    );
    const result: Record<string, Pattern[]> = {};
    for (const matcher of manifest.contributes.problemMatchers as Matcher[]) {
        result[matcher.name] = Array.isArray(matcher.pattern) ? matcher.pattern : [matcher.pattern];
    }
    return result;
}

/** Applies a multi-line pattern as VS Code does: consecutive lines, each
 * matching the next pattern, capturing the named groups. */
function problems(patterns: Pattern[], output: string): Record<string, string>[] {
    const lines = output.split("\n");
    const found: Record<string, string>[] = [];
    for (let start = 0; start + patterns.length <= lines.length; start++) {
        const problem: Record<string, string> = {};
        let index = 0;
        for (; index < patterns.length; index++) {
            const match = new RegExp(patterns[index].regexp).exec(lines[start + index]);
            if (!match) break;
            for (const key of [
                "file",
                "line",
                "column",
                "endLine",
                "endColumn",
                "message",
            ] as const) {
                const group = patterns[index][key];
                if (group !== undefined && match[group] !== undefined) problem[key] = match[group];
            }
        }
        if (index === patterns.length) found.push(problem);
    }
    return found;
}

// `sail --just-check --no-color`, Sail 0.20.2, pasted from real runs.
const typeError =
    "Type error:\nsub/bad.sail:5.16-24:\n5 |function f(x) = x + true\n  |                ^------^ (operator +)\n  | No possible overloading for (operator +)\n";
const syntaxError =
    "Syntax error:\nlex.sail:2.2-2:\n2 |  let @@@ = 1\n  |  ^\n  | current token: let\n";
const warning =
    "Warning: Incomplete pattern match statement at inc.sail:3.29-34:\n3 |function h(x : int) -> int = match x { 0 => 1 }\n  |                             ^---^\n  | \nThe following expression is unmatched: 1\n";
// The same error with the colour codes a terminal gets.
const coloured =
    "\u001b[93mType error\u001b[0m:\n\u001b[96mbad.sail\u001b[0m:5.16-24:\n5\u001b[96m |\u001b[0mfunction f(x) = x + true\n \u001b[91m |\u001b[0m                \u001b[91m^------^\u001b[0m \u001b[91m(operator +)\u001b[0m\n \u001b[91m |\u001b[0m No possible overloading for (operator +)\n";

test("the $sail problem matcher reads the compiler's error blocks", () => {
    const { sail, "sail-warning": sailWarning } = matchers();
    assert.deepEqual(problems(sail, typeError), [
        {
            file: "sub/bad.sail",
            line: "5",
            column: "16",
            endColumn: "24",
            message: "No possible overloading for (operator +)",
        },
    ]);
    assert.deepEqual(problems(sail, syntaxError), [
        { file: "lex.sail", line: "2", column: "2", endColumn: "2", message: "current token: let" },
    ]);
    assert.deepEqual(problems(sail, coloured), [
        {
            file: "bad.sail",
            line: "5",
            column: "16",
            endColumn: "24",
            message: "No possible overloading for (operator +)",
        },
    ]);
    assert.deepEqual(problems(sail, warning), [], "a warning is not an error");
    assert.deepEqual(problems(sailWarning, warning), [
        {
            file: "inc.sail",
            line: "3",
            column: "29",
            endColumn: "34",
            message: "Incomplete pattern match statement",
        },
    ]);
    assert.deepEqual(problems(sailWarning, typeError), []);
    // A span across lines carries both ends.
    assert.deepEqual(problems(sail, "Type error:\na.sail:5.16-7.2:\n5 |x\n  |^\n  | m\n")[0], {
        file: "a.sail",
        line: "5",
        column: "16",
        endLine: "7",
        endColumn: "2",
        message: "m",
    });
});
