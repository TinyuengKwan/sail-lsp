import * as assert from "node:assert/strict";
import { describe, it } from "node:test";
import { explicitSettings } from "../configuration";

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
