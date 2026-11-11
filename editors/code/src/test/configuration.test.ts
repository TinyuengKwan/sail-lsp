import * as assert from "node:assert/strict";
import { describe, it } from "node:test";
import { explicitSettings, settingRequiringRestart } from "../configuration";

describe("shared project configuration precedence", () => {
    it("omits defaults and preserves explicit false, null, and folder overrides", () => {
        const values: Record<string, object> = {
            "flycheck.enable": { globalValue: true, workspaceFolderValue: false },
            "workspace.target": { workspaceValue: null },
            "hover.docs": {},
        };
        assert.deepEqual(
            explicitSettings(Object.keys(values), (key) => values[key]),
            {
                flycheck: { enable: false },
                workspace: { target: null },
            },
        );
    });
});

describe("settings that take effect at server start", () => {
    it("name the first changed one and nothing else", () => {
        const changed = (keys: string[]) => settingRequiringRestart((key) => keys.includes(key));
        assert.equal(changed(["server.path"]), "server.path");
        assert.equal(changed(["diagnostics.enable", "server.extraEnv"]), "server.extraEnv");
        assert.equal(changed(["trace.server"]), "trace.server");
        assert.equal(changed(["diagnostics.enable", "flycheck.command"]), undefined);
        assert.equal(changed(["restartServerOnConfigChange"]), undefined);
    });
});
