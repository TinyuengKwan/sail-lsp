import * as assert from "node:assert/strict";
import * as vscode from "vscode";
import type * as lc from "vscode-languageclient/node";
import { joinLines, moveItem } from "../../editing";

function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((r) => (resolve = r));
    return { promise, resolve };
}

suite("editing request versions", () => {
    const source = "function first() = 1\nfunction second() = 2\n";
    for (const command of ["joinLines", "moveItem"] as const) {
        for (const changeDuringRequest of [false, true]) {
            test(`${command} ${changeDuringRequest ? "discards an outdated reply" : "applies a current reply"}`, async () => {
                const document = await vscode.workspace.openTextDocument({
                    language: "sail",
                    content: source,
                });
                const editor = await vscode.window.showTextDocument(document);
                const arrived = deferred<void>();
                const reply = deferred<lc.TextEdit[]>();
                const range = (r: lc.Range) =>
                    new vscode.Range(r.start.line, r.start.character, r.end.line, r.end.character);
                const client = {
                    sendRequest: () => {
                        arrived.resolve();
                        return reply.promise;
                    },
                    code2ProtocolConverter: {
                        asTextDocumentIdentifier: () => ({ uri: document.uri.toString() }),
                        asRange: (r: vscode.Range) => ({ start: r.start, end: r.end }),
                    },
                    protocol2CodeConverter: {
                        asRange: range,
                        asTextEdits: async (edits: lc.TextEdit[]) =>
                            edits.map(
                                (edit) => new vscode.TextEdit(range(edit.range), edit.newText),
                            ),
                    },
                } as unknown as lc.LanguageClient;
                try {
                    const operation =
                        command === "joinLines"
                            ? joinLines(client, editor)
                            : moveItem(client, editor, "Down");
                    await arrived.promise;
                    const prefix = "// typed while the request was waiting\n";
                    if (changeDuringRequest) {
                        assert.ok(
                            await editor.edit((edit) =>
                                edit.insert(new vscode.Position(0, 0), prefix),
                            ),
                        );
                    }
                    const moved = "function second() = 2\nfunction first() = 1\n";
                    reply.resolve([
                        command === "joinLines"
                            ? {
                                  range: {
                                      start: { line: 0, character: source.indexOf("\n") },
                                      end: { line: 1, character: 0 },
                                  },
                                  newText: " ",
                              }
                            : {
                                  range: {
                                      start: { line: 0, character: 0 },
                                      end: { line: 2, character: 0 },
                                  },
                                  newText: moved,
                              },
                    ]);
                    await operation;
                    assert.equal(
                        document.getText(),
                        changeDuringRequest
                            ? prefix + source
                            : command === "joinLines"
                              ? source.replace("\n", " ")
                              : moved,
                    );
                } finally {
                    await vscode.window.showTextDocument(document);
                    await vscode.commands.executeCommand(
                        "workbench.action.revertAndCloseActiveEditor",
                    );
                }
            });
        }
    }
});
