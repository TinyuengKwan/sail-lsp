import assert from "node:assert/strict";
import { test } from "node:test";
import {
    AnalysisContext,
    analysisSourceLinks,
    formatAnalysisContext,
    readAnalysisView,
} from "../analysisContext";

test("context explanation retains failed reload and distinct module configurations", () => {
    const result: AnalysisContext = {
        file: "file:///shared.sail",
        projectMember: true,
        projectLoadError: "Unknown module Missing",
        reloadPending: true,
        unsavedManifestPolicy: "Save the manifest to reload",
        contexts: ["A", "B"].map((module) => ({
            active: module === "A",
            name: "helper",
            resolved: false,
            resolution: "no selected declaration at this position",
            project: {
                manifest: `file:///${module}.sail_project`,
                module,
                requires: ["Lib"],
                roots: [module],
                variables: { MODE: module },
                target: null,
                includePaths: [],
            },
            visibleFiles: ["file:///lib.sail"],
            candidates: [{ uri: "file:///lib.sail", offset: 12, reason: "private declaration" }],
        })),
    };
    const text = formatAnalysisContext(result);
    for (const expected of [
        "Unknown module Missing",
        "Reload pending: true",
        "Module: A",
        "Module: B",
        "Source region: inactive",
        "private declaration",
        'Variables: {"MODE":"B"}',
    ]) {
        assert.ok(text.includes(expected), text);
    }
});

test("type explanations retain rejection evidence and navigable UTF-16 locations", () => {
    const location = {
        uri: "file:///project/类型.sail",
        range: { start: { line: 2, character: 7 }, end: { line: 2, character: 10 } },
    };
    const result: AnalysisContext = {
        file: "file:///main.sail",
        projectMember: true,
        projectLoadError: null,
        reloadPending: false,
        unsavedManifestPolicy: "saved model",
        contexts: [
            {
                active: true,
                name: "f",
                resolved: true,
                resolution: "selected",
                project: null,
                visibleFiles: [],
                candidates: [],
                types: {
                    inferred: "int",
                    mismatches: [
                        {
                            expected: "int",
                            actual: "bool",
                            location,
                            origins: [{ label: "Parameter type", location }],
                        },
                    ],
                    candidates: [
                        {
                            target: "f",
                            signature: "int -> int",
                            reason: "Argument types do not match",
                            status: "mismatch",
                            declaration: location,
                            arguments: [location],
                        },
                    ],
                    constraints: [
                        {
                            constraint: "'n > 0",
                            status: "unknown",
                            reason: "not established",
                            reasonCode: "not_established",
                            assumptions: ["'n >= 0"],
                            origin: location,
                        },
                    ],
                },
            },
        ],
    };
    const text = formatAnalysisContext(result);
    for (const expected of [
        "Expected: int",
        "Actual: bool",
        "Argument types do not match",
        "Constraint (not established)",
        "Assumptions:",
        "类型.sail:3:8",
    ])
        assert.ok(text.includes(expected), text);
    const links = analysisSourceLinks(result, text);
    assert.ok(links.length > 0);
    assert.ok(links.every((link) => link.location.range.start.character === 7));
    result.contexts[0].types!.constraints[0].origin = { ...location, uri: "command:evil" };
    assert.ok(
        analysisSourceLinks(result, formatAnalysisContext(result)).every((link) =>
            link.location.uri.startsWith("file:"),
        ),
    );
});

test("analysis reads wait for reloads, retry unrelated cancellation and never reuse changed source", async () => {
    let current = true;
    let ready = false;
    let calls = 0;
    const result = await readAnalysisView(
        () => current,
        () => ready,
        async () => {
            calls++;
            if (calls === 1) throw { code: -32802 };
            return "current evidence";
        },
        async () => {
            ready = true;
        },
    );
    assert.equal(result, "current evidence");
    assert.equal(calls, 2);
    calls = 0;
    const changed = await readAnalysisView(
        () => current,
        () => true,
        async () => {
            calls++;
            current = false;
            throw { code: -32802 };
        },
    );
    assert.equal(changed, undefined);
    assert.equal(calls, 1);
    calls = 0;
    const cancelled = await readAnalysisView(
        () => true,
        () => true,
        async () => {
            calls++;
            throw { code: -32800 };
        },
    );
    assert.equal(cancelled, undefined);
    assert.equal(calls, 1);
    await assert.rejects(
        readAnalysisView(
            () => true,
            () => true,
            async () => {
                throw new Error("real failure");
            },
        ),
    );
});
