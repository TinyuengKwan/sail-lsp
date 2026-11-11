import * as assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import * as path from "node:path";
import { describe, it } from "node:test";

// The server's legend is declared in one place, `lsp/semantic_tokens.rs`,
// with `define_semantic_token_types!` / `define_semantic_token_modifiers!`.
// VS Code colours a token type or modifier only if it is one LSP defines or
// one this extension contributes, so the two lists are compared here.
const root = path.resolve(__dirname, "..", "..", "..", "..", "..");
const legendSource = readFileSync(
    path.join(root, "crates", "sail-lsp", "src", "lsp", "semantic_tokens.rs"),
    "utf8",
);
const packageJson = JSON.parse(
    readFileSync(path.join(root, "editors", "code", "package.json"), "utf8"),
);

// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#semanticTokenTypes
const STANDARD_TYPES = new Set([
    "namespace",
    "type",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "event",
    "function",
    "method",
    "macro",
    "keyword",
    "modifier",
    "comment",
    "string",
    "number",
    "regexp",
    "operator",
    "decorator",
]);
const STANDARD_MODIFIERS = new Set([
    "declaration",
    "definition",
    "readonly",
    "static",
    "deprecated",
    "abstract",
    "async",
    "modification",
    "documentation",
    "defaultLibrary",
]);

/** `TYPE_PARAMETER` → `typeParameter`: how `lsp_types` spells the standard names. */
function camel(ident: string): string {
    return ident.toLowerCase().replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
}

/** The names a `define_semantic_token_*![ standard { .. } custom { .. } ]` block declares. */
function legend(macro: string): { standard: string[]; custom: Map<string, string | undefined> } {
    const block = legendSource.match(new RegExp(`${macro}!\\[([\\s\\S]*?)\\];`));
    assert.ok(block, `${macro}! invocation not found`);
    const standardBlock = block[1].match(/standard\s*\{([\s\S]*?)\}/);
    const customBlock = block[1].match(/custom\s*\{([\s\S]*?)\}/);
    assert.ok(standardBlock && customBlock, `${macro}! has no standard/custom sections`);
    const standard = Array.from(standardBlock[1].matchAll(/\b([A-Z][A-Z_]*)\b/g), (m) =>
        camel(m[1]),
    );
    const custom = new Map<string, string | undefined>();
    for (const m of customBlock[1].matchAll(
        /\(\s*[A-Z_]+\s*,\s*"([^"]+)"\s*\)(?:\s*=>\s*([A-Z_]+))?/g,
    )) {
        custom.set(m[1], m[2] ? camel(m[2]) : undefined);
    }
    return { standard, custom };
}

describe("semantic token legend", () => {
    const types = legend("define_semantic_token_types");
    const modifiers = legend("define_semantic_token_modifiers");
    const contributedTypes: Array<{ id: string; superType?: string }> =
        packageJson.contributes.semanticTokenTypes;
    const contributedModifiers: Array<{ id: string }> =
        packageJson.contributes.semanticTokenModifiers;
    const declaredTypes = new Set(contributedTypes.map((t) => t.id));
    const declaredModifiers = new Set(contributedModifiers.map((m) => m.id));

    it("is read from the server source, not assumed", () => {
        assert.ok(
            types.standard.length >= 10 && types.standard.includes("keyword"),
            `${types.standard}`,
        );
        assert.ok(types.custom.has("register"), `${[...types.custom.keys()]}`);
        assert.ok(modifiers.standard.includes("declaration"), `${modifiers.standard}`);
        assert.ok(modifiers.custom.has("controlFlow"), `${[...modifiers.custom.keys()]}`);
    });

    it("declares every type and modifier the server sends that LSP does not define", () => {
        for (const ty of types.standard) {
            assert.ok(
                STANDARD_TYPES.has(ty),
                `${ty} is in the standard section but LSP does not define it`,
            );
        }
        for (const [ty, fallback] of types.custom) {
            assert.ok(
                !STANDARD_TYPES.has(ty),
                `${ty} is standard; declare it in the standard section`,
            );
            assert.ok(declaredTypes.has(ty), `package.json semanticTokenTypes lacks ${ty}`);
            const superType = contributedTypes.find((t) => t.id === ty)?.superType;
            assert.equal(
                superType,
                fallback,
                `${ty}: superType and the server's standard fallback differ`,
            );
        }
        for (const mod of modifiers.standard) {
            assert.ok(
                STANDARD_MODIFIERS.has(mod),
                `${mod} is in the standard section but LSP does not define it`,
            );
        }
        for (const mod of modifiers.custom.keys()) {
            assert.ok(
                declaredModifiers.has(mod),
                `package.json semanticTokenModifiers lacks ${mod}`,
            );
        }
    });

    it("declares nothing the server never sends", () => {
        for (const ty of declaredTypes) {
            assert.ok(
                types.custom.has(ty),
                `package.json declares ${ty}; the legend does not have it`,
            );
        }
        for (const mod of declaredModifiers) {
            assert.ok(
                modifiers.custom.has(mod),
                `package.json declares ${mod}; the legend does not have it`,
            );
        }
    });

    it("maps only legend types and modifiers to TextMate scopes", () => {
        const scopes: Array<{ language: string; scopes: Record<string, string[]> }> =
            packageJson.contributes.semanticTokenScopes;
        const entry = scopes.find((s) => s.language === "sail");
        assert.ok(entry, "no semanticTokenScopes entry for sail");
        const knownTypes = new Set([...types.standard, ...types.custom.keys()]);
        const knownModifiers = new Set([...modifiers.standard, ...modifiers.custom.keys()]);
        for (const [selector, textmate] of Object.entries(entry.scopes)) {
            const [ty, ...mods] = selector.split(".");
            assert.ok(ty === "*" || knownTypes.has(ty), `${selector}: ${ty} is not in the legend`);
            for (const mod of mods) {
                assert.ok(knownModifiers.has(mod), `${selector}: ${mod} is not in the legend`);
            }
            assert.ok(
                textmate.length > 0 && textmate.every((s) => /^[a-z][a-z0-9.-]*$/.test(s)),
                selector,
            );
        }
        assert.ok("register" in entry.scopes, "the custom register type has no scope mapping");
    });
});
