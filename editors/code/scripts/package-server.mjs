import { chmodSync, copyFileSync, existsSync, mkdirSync, rmSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const [, , targetOs, targetArch, rustTarget] = process.argv;

if (!targetOs || !targetArch || !rustTarget) {
    console.error(
        "usage: node scripts/package-server.mjs <os> <arch> <rust-target>",
    );
    process.exit(1);
}

const root = path.resolve(import.meta.dirname, "..", "..", "..");
const extensionRoot = path.resolve(import.meta.dirname, "..");
const targetDir = path.join(root, "target", "vscode", `${targetOs}-${targetArch}`);
const serverDir = path.join(extensionRoot, "server");
const sourceName = targetOs === "windows" ? "sail-lsp.exe" : "sail-lsp";
const sourcePath = path.join(targetDir, rustTarget, "release", sourceName);
const destinationPath = path.join(serverDir, sourceName);

if (!existsSync(sourcePath)) {
    console.error(`missing built server binary: ${sourcePath}`);
    // `--features vendored` builds Z3 from source and links it statically.
    // Without it the binary needs the exact `libz3.so.4.x` it was built
    // against, which the user's machine will not have: the previously
    // bundled server was built against 4.16 and died with
    // `libz3.so.4.16 => not found` on a machine carrying 4. A packaged
    // server may depend on libc and libstdc++ and nothing else.
    console.error(
        `build it first, for example: cargo build --release -p sail-lsp --features vendored --target ${rustTarget} --target-dir target/vscode/${targetOs}-${targetArch}`,
    );
    process.exit(1);
}

rmSync(serverDir, { recursive: true, force: true });
mkdirSync(serverDir, { recursive: true });
copyFileSync(sourcePath, destinationPath);

if (targetOs !== "windows") {
    chmodSync(destinationPath, 0o755);
}

console.log(`bundled ${destinationPath}`);
