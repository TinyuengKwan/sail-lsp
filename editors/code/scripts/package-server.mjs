import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync, chmodSync, copyFileSync, existsSync, mkdirSync, rmSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const args = process.argv.slice(2);
const distribution = args[0] === "--distribution" ? args[1] : undefined;
const [targetOs, targetArch, rustTarget] = args;
if (!distribution && (!targetOs || !targetArch || !rustTarget)) {
    console.error("usage: package-server.mjs --distribution DIR (verified server payload) OR <os> <arch> <rust-target>");
    process.exit(1);
}

const root = path.resolve(import.meta.dirname, "..", "..", "..");
const extensionRoot = path.resolve(import.meta.dirname, "..");
const targetDir = path.join(root, "target", "vscode", `${targetOs === "windows" ? "win32" : targetOs}-${targetArch}`);
const serverDir = path.join(extensionRoot, "server");
const manifest = distribution ? JSON.parse(readFileSync(path.join(distribution, "manifest.json"), "utf8")) : undefined;
const sourceName = manifest?.binary ?? (targetOs === "windows" ? "sail-lsp.exe" : "sail-lsp");
const sourcePath = distribution ? path.join(distribution, sourceName) : path.join(targetDir, rustTarget, "release", sourceName);
const destinationPath = path.join(serverDir, sourceName);
if (manifest && (manifest.schemaVersion !== 1 || manifest.solver !== "z3-vendored" ||
    !["sail-lsp", "sail-lsp.exe"].includes(sourceName) ||
    createHash("sha256").update(readFileSync(sourcePath)).digest("hex") !== manifest.binarySha256)) {
    throw new Error("Invalid distribution manifest or mismatched server checksum");
}

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

if (!manifest) {
    const metadata = JSON.parse(execFileSync(sourcePath, ["--version", "--json"], { encoding: "utf8" }));
    const version = JSON.parse(readFileSync(path.join(extensionRoot, "package.json"), "utf8")).version;
    if (metadata.solver !== "z3-vendored" || metadata.target !== rustTarget || metadata.version !== version ||
        (targetOs === "windows" && !metadata.crtStatic)) {
        throw new Error("Package a matching native server built with vendored Z3 (and +crt-static on Windows)");
    }
}

rmSync(serverDir, { recursive: true, force: true });
mkdirSync(serverDir, { recursive: true });
copyFileSync(sourcePath, destinationPath);

if (!sourceName.endsWith(".exe")) {
    chmodSync(destinationPath, 0o755);
}

console.log(`bundled ${destinationPath}`);
