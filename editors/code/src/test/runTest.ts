import * as os from "node:os";
import * as path from "node:path";
import process from "node:process";
import { setTimeout } from "node:timers/promises";
import { downloadAndUnzipVSCode, runTests } from "@vscode/test-electron";

async function downloadTestEditor(): Promise<string> {
    let lastError: unknown;
    for (let attempt = 1; attempt <= 3; attempt++) {
        try {
            return await downloadAndUnzipVSCode({
                version: process.env.SAIL_LSP_TEST_VSCODE_VERSION ?? "stable",
                timeout: 60_000,
            });
        } catch (error) {
            lastError = error;
            if (attempt < 3) {
                console.warn(`VS Code download attempt ${attempt} failed; retrying.`, error);
                await setTimeout(2000);
            }
        }
    }
    throw lastError;
}

async function main(): Promise<void> {
    const extensionDevelopmentPath = path.resolve(__dirname, "../../..");
    const extensionTestsPath = path.resolve(__dirname, "./suite/index.js");
    const vscodeExecutablePath = await downloadTestEditor();

    await runTests({
        vscodeExecutablePath,
        extensionDevelopmentPath,
        extensionTestsPath,
        extensionTestsEnv: {
            SAIL_LSP_TEST_NODE: process.execPath,
            SAIL_LSP_TEST_TMP: os.tmpdir(),
            SAIL_LSP_TEST_SERVER:
                process.env.SAIL_LSP_TEST_SERVER ??
                path.resolve(
                    extensionDevelopmentPath,
                    "../../target/debug",
                    process.platform === "win32" ? "sail-lsp.exe" : "sail-lsp",
                ),
        },
        launchArgs: [
            path.resolve(extensionDevelopmentPath, "test-fixture"),
            "--disable-gpu",
            "--disable-workspace-trust",
            ...(process.platform === "linux" ? ["--ozone-platform=headless"] : []),
        ],
    });
}

void main().catch((error) => {
    console.error(error);
    process.exit(1);
});
