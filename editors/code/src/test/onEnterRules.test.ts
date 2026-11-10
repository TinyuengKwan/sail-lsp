import * as assert from "node:assert/strict";
import { describe, it } from "node:test";
import { onEnterRule } from "../onEnterRules";

describe("block comment Enter rules", () => {
    it("open, continue and close a block comment", () => {
        const opened = onEnterRule("/* note", "");
        assert.equal(opened?.appendText, " * ");
        assert.equal(opened?.indent, "none");
        const closed = onEnterRule("  /** doc", "*/");
        assert.equal(closed?.appendText, " * ");
        assert.equal(closed?.indent, "indentOutdent", "the closer moves to its own line");
        assert.equal(onEnterRule(" * more", "")?.appendText, "* ");
        assert.equal(onEnterRule(" */", "")?.removeText, 1);
        assert.equal(onEnterRule("/* one line */", ""), undefined, "a closed comment ends it");
    });

    it("leave line and doc comments to the server", () => {
        for (const line of ["// plain", "/// doc", "  /// indented doc", "val x : int"]) {
            assert.equal(onEnterRule(line, ""), undefined, line);
        }
    });
});
