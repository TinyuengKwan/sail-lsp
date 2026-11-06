import assert from "node:assert/strict";
import { test } from "node:test";
import { collectDiagnosticReport } from "../diagnosticReportData";
test("reports survive server failures and exclude environment and source data", async () => {
    const client = {
        vscodeVersion: "test",
        extensionVersion: "test",
        platform: "test",
        arch: "test",
        osRelease: "test",
        startupError: "missing executable",
        extraEnv: { secret: "SECRET_VALUE" },
    };
    const document = {
        uri: "file:///main.sail",
        languageId: "sail",
        version: 1,
        dirty: true,
        text: "PRIVATE_SOURCE",
    };
    const report = await collectDiagnosticReport(client, document);
    assert.match(report.serverError!, /not running/);
    assert.equal(report.client.startupError, "missing executable");
    assert.ok(!JSON.stringify(report).includes("SECRET_VALUE"));
    assert.ok(!JSON.stringify(report).includes("PRIVATE_SOURCE"));
    const failed = await collectDiagnosticReport(client, document, async () => {
        throw new Error("transport closed");
    });
    assert.match(failed.serverError!, /transport closed/);
    assert.equal(failed.activeDocument!.uri, document.uri);
});
