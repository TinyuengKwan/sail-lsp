import { test } from "node:test";
import * as assert from "node:assert/strict";
import { byteOffsetToCharOffset, parseSyntaxTreeLine, viewUriParts } from "../syntaxTree";

// Lines as `sail-lsp/viewSyntaxTree` prints them.
test("syntax tree lines name a kind and a byte range; other lines do not", () => {
    assert.deepEqual(parseSyntaxTreeLine("SOURCE_FILE@0..52"), {
        kind: "SOURCE_FILE",
        start: 0,
        end: 52,
    });
    assert.deepEqual(parseSyntaxTreeLine('      IDENT@20..37 "structural_sample"'), {
        kind: "IDENT",
        start: 20,
        end: 37,
    });
    assert.equal(parseSyntaxTreeLine("// 1 parse error(s)"), undefined);
    assert.equal(parseSyntaxTreeLine(""), undefined);
    assert.equal(parseSyntaxTreeLine("(empty file)"), undefined);
});

test("byte offsets become string indices across multi-byte text", () => {
    const text = "val é : int /* 😀 */ x";
    assert.equal(byteOffsetToCharOffset(text, 0), 0);
    // `é` is two bytes and one UTF-16 unit; what follows shifts by one.
    assert.equal(byteOffsetToCharOffset(text, 4), 4);
    assert.equal(byteOffsetToCharOffset(text, 6), 5);
    assert.equal(
        text.slice(byteOffsetToCharOffset(text, 7), byteOffsetToCharOffset(text, 10)),
        ": i",
    );
    // `😀` is four bytes and two UTF-16 units.
    const emoji = Buffer.from(text.slice(0, text.indexOf("😀")), "utf8").length;
    assert.equal(
        text.slice(byteOffsetToCharOffset(text, emoji), byteOffsetToCharOffset(text, emoji + 4)),
        "😀",
    );
    assert.equal(byteOffsetToCharOffset(text, 1000), text.length);
});

test("a view is bound to its source", () => {
    assert.deepEqual(viewUriParts("syntaxTree", "file:///work/model/cpu.sail"), {
        scheme: "sail-lsp-syntax-tree",
        path: "/cpu.sail.syntax-tree.txt",
        query: "file:///work/model/cpu.sail",
    });
    assert.equal(viewUriParts("hir", "file:///w/a%20b.sail").path, "/a b.sail.hir.txt");
    assert.equal(viewUriParts("itemTree", "file:///w/x.sail").scheme, "sail-lsp-item-tree");
});
