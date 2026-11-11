// Send explicit values only: editor defaults must not mask repository settings.
export interface InspectedSetting {
    globalValue?: unknown;
    workspaceValue?: unknown;
    workspaceFolderValue?: unknown;
    globalLanguageValue?: unknown;
    workspaceLanguageValue?: unknown;
    workspaceFolderLanguageValue?: unknown;
}

export function explicitSettings(
    keys: string[],
    inspect: (key: string) => InspectedSetting | undefined,
): Record<string, unknown> {
    const result: Record<string, Record<string, unknown>> = {};
    for (const fullKey of keys) {
        const key = fullKey.replace(/^sail-lsp\./, "");
        const setting = inspect(key);
        if (!setting) continue;
        const candidates = [
            setting.globalValue,
            setting.workspaceValue,
            setting.workspaceFolderValue,
            setting.globalLanguageValue,
            setting.workspaceLanguageValue,
            setting.workspaceFolderLanguageValue,
        ];
        const value = candidates.reverse().find((value) => value !== undefined);
        if (value === undefined) continue;
        const dot = key.indexOf(".");
        if (dot === -1) continue;
        (result[key.slice(0, dot)] ??= {})[key.slice(dot + 1)] = value;
    }
    return result;
}

/** Settings the extension reads when it starts the server and nowhere else,
 * so a change to one takes effect through a restart — rust-analyzer's
 * `requiresServerReloadOpts`. */
export const SERVER_RESTART_SETTINGS = ["server.path", "server.extraEnv", "trace.server"];

/** The first setting a configuration change touched that needs a restart. */
export function settingRequiringRestart(affects: (key: string) => boolean): string | undefined {
    return SERVER_RESTART_SETTINGS.find(affects);
}
