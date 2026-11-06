import assert from "node:assert/strict";
import { test } from "node:test";
import { graphChildren, isCycle, ProjectGraphEntry } from "../projectGraph";

const project: ProjectGraphEntry = {
    manifest: "file:///model/sail.proj",
    path: "/model/sail.proj",
    preferred: true,
    variables: { MODE: "true" },
    roots: ["Main"],
    modules: [
        {
            name: "Main",
            active: true,
            requires: ["Left", "Right"],
            files: ["file:///model/main.sail"],
        },
        { name: "Left", active: true, requires: ["Shared"], files: [] },
        { name: "Right", active: true, requires: ["Shared"], files: [] },
        { name: "Shared", active: true, requires: [], files: ["file:///model/shared.sail"] },
        { name: "Unused", active: false, requires: [], files: [] },
    ],
};

test("dependency edges retain shared modules and file navigation targets", () => {
    const roots = graphChildren({ kind: "project", project });
    assert.equal(roots.length, 5); // Inactive declarations remain inspectable.
    const main = graphChildren(roots[0]);
    assert.deepEqual(main[2], { kind: "file", uri: "file:///model/main.sail" });
    for (const branch of main.slice(0, 2)) {
        const shared = graphChildren(branch)[0];
        assert.equal(shared.kind === "module" && shared.name, "Shared");
        assert.deepEqual(graphChildren(shared), [
            { kind: "file", uri: "file:///model/shared.sail" },
        ]);
    }
});

test("cycle expansion stops at the repeated node rather than recursing forever", () => {
    const cyclic = {
        ...project,
        modules: [
            { name: "A", active: true, requires: ["B"], files: [] },
            { name: "B", active: true, requires: ["A"], files: [] },
        ],
    };
    const a = graphChildren({ kind: "project", project: cyclic })[0];
    const b = graphChildren(a)[0];
    const again = graphChildren(b)[0];
    assert.equal(isCycle(again), true);
    assert.deepEqual(graphChildren(again), []);
});

test("same-name modules in different projects retain their own edges", () => {
    const other = {
        ...project,
        modules: [{ name: "Main", active: true, requires: [], files: ["file:///other/main.sail"] }],
    };
    const root = graphChildren({ kind: "project", project: other })[0];
    assert.deepEqual(graphChildren(root), [{ kind: "file", uri: "file:///other/main.sail" }]);
    assert.equal(graphChildren({ kind: "project", project }).length, 5);
});
