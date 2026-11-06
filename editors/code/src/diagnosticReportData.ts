/** Explicit fields prevent accidental export of environment or buffer data. */
export interface ReportClientInfo {
    vscodeVersion: string;
    extensionVersion: string;
    platform: string;
    arch: string;
    osRelease: string;
    remoteName?: string;
    serverPath?: string;
    startupError?: string;
}
export interface ReportDocument {
    uri: string;
    languageId: string;
    version: number;
    dirty: boolean;
}
export async function collectDiagnosticReport(
    client: ReportClientInfo,
    document: ReportDocument | undefined,
    request?: (params: unknown) => PromiseLike<unknown>,
) {
    let server: unknown;
    let serverError: string | undefined;
    if (request) {
        try {
            server = await request({ textDocument: document ? { uri: document.uri } : undefined });
        } catch (error) {
            serverError = String(error);
        }
    } else {
        serverError = "The Sail language server is not running";
    }
    return {
        schemaVersion: 1,
        client: {
            vscodeVersion: client.vscodeVersion,
            extensionVersion: client.extensionVersion,
            platform: client.platform,
            arch: client.arch,
            osRelease: client.osRelease,
            remoteName: client.remoteName,
            serverPath: client.serverPath,
            startupError: client.startupError,
        },
        activeDocument: document
            ? {
                  uri: document.uri,
                  languageId: document.languageId,
                  version: document.version,
                  dirty: document.dirty,
              }
            : undefined,
        server,
        serverError,
    };
}
