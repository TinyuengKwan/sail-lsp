import { execFileSync } from "node:child_process";
import path from "node:path";
import process from "node:process";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";

const require = createRequire(import.meta.url);
const extension = path.resolve(import.meta.dirname, "..");
const root = path.resolve(extension, "../..");
const host = execFileSync("rustc", ["-vV"], { encoding: "utf8" })
    .split(/\r?\n/).find((line) => line.startsWith("host: "))?.slice(6).trim();
const targets = JSON.parse(readFileSync(path.join(root, "distribution/targets.json"), "utf8"));
const target = targets.find((entry) => entry.target === host);
if (!target) throw new Error(`No VSIX mapping for Rust host ${host}`);
const [platform, architecture] = target.vsix.split("-");
const os = platform === "win32" ? "windows" : platform;
const env = { ...process.env };
if (process.platform === "win32") {
    env.RUSTFLAGS = `${env.RUSTFLAGS ?? ""} -C target-feature=+crt-static`.trim();
    env.CFLAGS = `${env.CFLAGS ?? ""} /utf-8`.trim();
    env.CXXFLAGS = `${env.CXXFLAGS ?? ""} /utf-8`.trim();
}
const run = (program, args, cwd) => execFileSync(program, args, { cwd, env, stdio: "inherit" });
run("cargo", ["build", "--release", "--locked", "-p", "sail-lsp", "--features", "vendored",
    "--target", host, "--target-dir", path.join(root, "target/vscode", target.vsix)], root);
run(process.execPath, [path.join(extension, "scripts/package-server.mjs"), os, architecture, host], extension);
run(process.execPath, [require.resolve("@vscode/vsce/vsce"), "package", "--target", target.vsix,
    "--out", `sail-lsp-${target.vsix}.vsix`, "--allow-missing-repository"], extension);
