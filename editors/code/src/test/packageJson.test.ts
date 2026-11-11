import { test } from "node:test";
import * as assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";

type Command = { command: string; title: string; category?: string };
type Setting = {
    scope?: string;
    description?: string;
    markdownDescription?: string;
    enum?: unknown[];
    enumDescriptions?: string[];
};
const manifest = JSON.parse(fs.readFileSync(path.join(__dirname, "../../../package.json"), "utf8"));
const contributes = manifest.contributes as {
    commands: Command[];
    configuration: { properties: Record<string, Setting> };
    menus: { commandPalette: { command: string; when?: string }[] };
    walkthroughs: { steps: { media: { markdown: string } }[] }[];
    taskDefinitions: { type: string; required: string[]; properties: Record<string, unknown> }[];
    problemMatchers: { name: string }[];
};

test("every command is in the Sail category and keeps the category out of its title", () => {
    for (const command of contributes.commands) {
        assert.equal(command.category, "Sail", command.command);
        assert.ok(!/^Sail:/.test(command.title), `${command.command} has "Sail:" in its title`);
    }
    const known = new Set(contributes.commands.map((command) => command.command));
    for (const entry of contributes.menus.commandPalette) {
        assert.ok(known.has(entry.command), `${entry.command} is in the palette menu only`);
        assert.ok(entry.when, `${entry.command} has no when clause in the palette menu`);
    }
});

test("every setting has a markdown description and a scope, and every enum its descriptions", () => {
    for (const [key, setting] of Object.entries(contributes.configuration.properties)) {
        assert.ok(setting.markdownDescription, `${key} has no markdownDescription`);
        assert.equal(setting.description, undefined, `${key} has both description kinds`);
        assert.ok(
            ["resource", "window", "machine-overridable"].includes(setting.scope ?? ""),
            `${key} has scope ${setting.scope}`,
        );
        if (setting.enum) {
            assert.equal(
                setting.enumDescriptions?.length,
                setting.enum.length,
                `${key} describes ${setting.enumDescriptions?.length} of ${setting.enum.length} values`,
            );
        }
    }
    // Process-wide settings cannot differ per folder.
    for (const key of [
        "sail-lsp.cache.parseCapacity",
        "sail-lsp.workspace.primeCachesNumThreads",
    ]) {
        assert.equal(contributes.configuration.properties[key].scope, "window", key);
    }
});

test("the task type, problem matchers and walkthrough media are declared and present", () => {
    const sail = contributes.taskDefinitions.find((definition) => definition.type === "sail");
    assert.deepEqual(sail?.required, ["command"]);
    assert.deepEqual(Object.keys(sail!.properties).sort(), [
        "args",
        "command",
        "cwd",
        "env",
        "script",
    ]);
    assert.deepEqual(contributes.problemMatchers.map((matcher) => matcher.name).sort(), [
        "sail",
        "sail-warning",
    ]);
    for (const walkthrough of contributes.walkthroughs) {
        for (const step of walkthrough.steps) {
            const media = path.join(__dirname, "../../..", step.media.markdown);
            assert.ok(fs.existsSync(media), `${step.media.markdown} is missing`);
        }
    }
});
