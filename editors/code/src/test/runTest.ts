import * as path from "node:path";
import process from "node:process";
import { runTests } from "@vscode/test-electron";

async function main(): Promise<void> {
    const extensionDevelopmentPath = path.resolve(__dirname, "../../..");
    const extensionTestsPath = path.resolve(__dirname, "./suite/index.js");

    await runTests({
        extensionDevelopmentPath,
        extensionTestsPath,
        extensionTestsEnv: {
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
            ...(process.platform === "linux" ? ["--ozone-platform=headless"] : []),
        ],
    });
}

void main().catch((error) => {
    console.error(error);
    process.exit(1);
});
