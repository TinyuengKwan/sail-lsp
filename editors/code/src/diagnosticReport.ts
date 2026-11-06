import * as vscode from "vscode";
import * as os from "node:os";
import type { LanguageClient } from "vscode-languageclient/node";
import { collectDiagnosticReport } from "./diagnosticReportData";

/** Save locally, including client metadata when the server cannot answer.
 * No buffer contents, process environment or server.extraEnv are collected. */
export function registerDiagnosticReport(
    context: vscode.ExtensionContext,
    getClient: () => LanguageClient | undefined,
    localInfo: () => { serverPath?: string; startupError?: string },
): void {
    context.subscriptions.push(
        vscode.commands.registerCommand(
            "sail-lsp.exportDiagnosticReport",
            async (destination?: vscode.Uri) => {
                const target =
                    destination ??
                    (await vscode.window.showSaveDialog({
                        saveLabel: "Export Sail diagnostic report",
                        filters: { JSON: ["json"] },
                        defaultUri: vscode.Uri.joinPath(
                            vscode.Uri.file(os.homedir()),
                            "sail-lsp-report.json",
                        ),
                    }));
                if (!target) return undefined;
                const editor = vscode.window.activeTextEditor;
                const activeDocument = editor
                    ? {
                          uri: editor.document.uri.toString(),
                          languageId: editor.document.languageId,
                          version: editor.document.version,
                          dirty: editor.document.isDirty,
                      }
                    : undefined;
                const client = getClient();
                const report = await collectDiagnosticReport(
                    {
                        vscodeVersion: vscode.version,
                        extensionVersion: context.extension.packageJSON.version as string,
                        platform: process.platform,
                        arch: process.arch,
                        osRelease: os.release(),
                        remoteName: vscode.env.remoteName,
                        ...localInfo(),
                    },
                    activeDocument,
                    client
                        ? (params) => client.sendRequest("sail-lsp/diagnosticReport", params)
                        : undefined,
                );
                await vscode.workspace.fs.writeFile(
                    target,
                    Buffer.from(JSON.stringify(report, null, 2) + "\n"),
                );
                return report;
            },
        ),
    );
}
