import * as assert from "node:assert/strict";
import { describe, it } from "node:test";
import { renderHoverActions } from "../hoverActions";

describe("hover actions", () => {
    it("render each command as a proxy link and keep the commands by index", () => {
        const implementation = {
            title: "1 implementation",
            tooltip: "Go to implementations",
            command: "sail-lsp.showReferences",
            arguments: ["file:///a.sail", { line: 0, character: 4 }, []],
        };
        const reference = {
            title: "2 references",
            tooltip: "Go to references",
            command: "sail-lsp.showReferences",
            arguments: ["file:///a.sail", { line: 0, character: 4 }, []],
        };
        const { markdown, commands } = renderHoverActions(
            [{ commands: [implementation] }, { title: "Also", commands: [reference] }],
            "sail-lsp.hoverRefCommandProxy",
        );
        assert.equal(
            markdown,
            "[1 implementation](command:sail-lsp.hoverRefCommandProxy?0 'Go to implementations') | " +
                "Also [2 references](command:sail-lsp.hoverRefCommandProxy?1 'Go to references')",
        );
        assert.deepEqual(commands, [implementation, reference]);
        assert.equal(renderHoverActions([], "x").markdown, "");
    });
});
