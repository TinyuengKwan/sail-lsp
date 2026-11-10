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
