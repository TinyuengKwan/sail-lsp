import * as assert from "node:assert/strict";
import { describe, it } from "node:test";
import { hasSnippetEdits, isSnippetEdit, unindentSnippet } from "../snippets";

const range = { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } };

describe("snippet text edits", () => {
    it("expands only an edit the server marked that has something to expand", () => {
        assert.ok(isSnippetEdit({ range, newText: "f($0)", insertTextFormat: 2 }));
        assert.ok(isSnippetEdit({ range, newText: "val ${1:_}", insertTextFormat: 2 }));
        // `$include` is Sail, not a tabstop; a plain edit never expands.
        assert.ok(!isSnippetEdit({ range, newText: '$include "x.sail"', insertTextFormat: 2 }));
        assert.ok(!isSnippetEdit({ range, newText: "f($0)", insertTextFormat: 1 }));
        assert.ok(!isSnippetEdit({ range, newText: "f($0)" }));
        assert.ok(
            hasSnippetEdits({
                changes: {
                    "file:///a.sail": [{ range, newText: "x", insertTextFormat: 1 }],
                    "file:///b.sail": [{ range, newText: "$0", insertTextFormat: 2 }],
                },
            }),
        );
        assert.ok(!hasSnippetEdits({ documentChanges: [] }));
    });

    it("drops the indentation VS Code adds back to continuation lines", () => {
        assert.equal(
            unindentSnippet("{\n    let x = $0;\n  }", "  "),
            "{\n  let x = $0;\n}",
            "every line after the first loses one level",
        );
        assert.equal(unindentSnippet("  a\n  b", ""), "  a\n  b", "nothing to remove");
        assert.equal(unindentSnippet("single $0", "  "), "single $0", "one line is left alone");
        assert.equal(
            unindentSnippet("\n\nval helper : _ -> ${1:_}\n", "  "),
            "\n\nval helper : _ -> ${1:_}\n",
            "lines not indented by the server are left alone",
        );
    });
});
