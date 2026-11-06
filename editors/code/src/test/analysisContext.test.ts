import assert from "node:assert/strict";
import { test } from "node:test";
import { AnalysisContext, formatAnalysisContext } from "../analysisContext";

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
