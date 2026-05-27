import * as path from "node:path";
import process from "node:process";
import { runTests } from "@vscode/test-electron";

async function main(): Promise<void> {
    const extensionDevelopmentPath = path.resolve(__dirname, "../../..");
    const extensionTestsPath = path.resolve(__dirname, "./suite/index.js");

    await runTests({
        extensionDevelopmentPath,
        extensionTestsPath,
        launchArgs: [path.resolve(extensionDevelopmentPath, "test-fixture")],
    });
}

void main().catch((error) => {
    console.error(error);
    process.exit(1);
});
