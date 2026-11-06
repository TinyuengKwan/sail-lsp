import assert from "node:assert/strict";
import { test } from "node:test";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { managedServerPath } from "../installation";

test("managed installation selects the registered binary and exposes broken registrations", () => {
    const temp = fs.mkdtempSync(path.join(os.tmpdir(), "sail-installation-"));
    try {
        const env = { SAIL_LSP_CONFIG_HOME: temp };
        assert.equal(managedServerPath(env, process.platform, temp), undefined);
        const binary = path.join(temp, process.platform === "win32" ? "server.exe" : "server");
        fs.writeFileSync(binary, "test", { mode: 0o755 });
        fs.writeFileSync(
            path.join(temp, "vscode.json"),
            JSON.stringify({ schemaVersion: 1, path: binary }),
        );
        assert.equal(managedServerPath(env, process.platform, temp), binary);
        fs.unlinkSync(binary);
        assert.throws(
            () => managedServerPath(env, process.platform, temp),
            /missing or not executable/,
        );
        fs.writeFileSync(
            path.join(temp, "vscode.json"),
            JSON.stringify({ schemaVersion: 2, path: binary }),
        );
        assert.throws(() => managedServerPath(env, process.platform, temp), /Invalid managed/);
    } finally {
        fs.rmSync(temp, { recursive: true, force: true });
    }
});
