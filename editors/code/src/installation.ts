/** Shared registration written by `sail-lsp install --editor vscode`.
 * Explicit server.path settings retain priority over this registration. */
import * as fs from "node:fs";
import * as path from "node:path";

export function managedServerPath(
    env: NodeJS.ProcessEnv,
    platform: NodeJS.Platform,
    home: string,
): string | undefined {
    const config =
        env.SAIL_LSP_CONFIG_HOME ??
        (platform === "win32"
            ? path.join(env.APPDATA ?? path.join(home, "AppData", "Roaming"), "sail-lsp")
            : path.join(env.XDG_CONFIG_HOME ?? path.join(home, ".config"), "sail-lsp"));
    const registration = path.join(config, "vscode.json");
    if (!fs.existsSync(registration)) return undefined;
    const value: unknown = JSON.parse(fs.readFileSync(registration, "utf8"));
    if (
        typeof value !== "object" ||
        value === null ||
        !("schemaVersion" in value) ||
        value.schemaVersion !== 1 ||
        !("path" in value) ||
        typeof value.path !== "string" ||
        !path.isAbsolute(value.path)
    ) {
        throw new Error(`Invalid managed Sail installation registration: ${registration}`);
    }
    // A broken managed installation is visible instead of falling back to an
    // older bundled executable and silently running a different version.
    try {
        fs.accessSync(value.path, fs.constants.X_OK);
    } catch {
        throw new Error(
            `Managed Sail server is missing or not executable: ${value.path}. Reinstall it or set sail-lsp.server.path.`,
        );
    }
    return value.path;
}
