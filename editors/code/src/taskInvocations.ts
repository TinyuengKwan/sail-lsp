import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { createHash } from "node:crypto";

/** Private, bounded payloads keep VS Code's task variable resolver away from
 * compiler arguments and typed model values. Recent tasks remain rerunnable. */
export class TaskInvocations {
    private root: string | undefined;
    private readonly files = new Map<string, string>();

    prepare(command: unknown): { key: string; file: string } {
        const data = JSON.stringify({ version: 1, command });
        const key = createHash("sha256").update(data).digest("hex");
        let file = this.files.get(key);
        if (!file) {
            this.root ??= fs.mkdtempSync(path.join(os.tmpdir(), "sail-lsp-tasks-"));
            file = path.join(this.root, `${key}.json`);
            fs.writeFileSync(file, data, { flag: "wx", mode: 0o600 });
        }
        this.files.delete(key);
        this.files.set(key, file);
        return { key, file };
    }

    prune(protectedKeys: ReadonlySet<string>): void {
        for (const [key, file] of this.files) {
            if (this.files.size <= 64) break;
            if (protectedKeys.has(key)) continue;
            fs.rmSync(file, { force: true });
            this.files.delete(key);
        }
    }

    dispose(): void {
        if (this.root) fs.rmSync(this.root, { recursive: true, force: true });
        this.files.clear();
        this.root = undefined;
    }
}
