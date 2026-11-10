export interface SourceLocation {
    uri: string;
    range: { start: { line: number; character: number }; end: { line: number; character: number } };
}
export interface TypeExplanation {
    inferred: string | null;
    mismatches: {
        expected: string;
        actual: string;
        location: SourceLocation | null;
        origins: { label: string; location: SourceLocation | null }[];
    }[];
    candidates: {
        target: string;
        signature: string;
        reason: string;
        status: "mismatch" | "arity" | "refuted" | "unknown";
        declaration: SourceLocation | null;
        arguments: SourceLocation[];
        argumentIndices?: number[];
    }[];
    constraints: {
        constraint: string;
        status: "unknown" | "refuted";
        reason: string | null;
        reasonCode: string | null;
        assumptions: string[];
        origin: SourceLocation | null;
    }[];
}

export interface AnalysisContext {
    file: string;
    projectMember: boolean | null;
    projectLoadError: string | null;
    reloadPending: boolean;
    unsavedManifestPolicy: string;
    contexts: {
        types?: TypeExplanation | null;
        active: boolean;
        name: string | null;
        resolved: boolean;
        resolution: string;
        project: {
            manifest: string;
            module: string | null;
            requires: string[];
            roots: string[];
            variables: Record<string, unknown>;
            target: string | null;
            includePaths: string[];
        } | null;
        visibleFiles: string[];
        candidates: { uri: string; offset: number; reason: string }[];
    }[];
}

/** Plain text: names and paths from project files never become executable links. */
export function formatAnalysisContext(result: AnalysisContext): string {
    const lines = [
        `Analysis at ${result.file}`,
        `Project member: ${result.projectMember === null ? "not configured" : result.projectMember}`,
        `Reload pending: ${result.reloadPending}`,
        result.unsavedManifestPolicy,
    ];
    if (result.projectLoadError) lines.push(`Project load error: ${result.projectLoadError}`);
    for (const context of result.contexts) {
        lines.push("", `Name: ${context.name ?? "(no name at this position)"}`);
        lines.push(`Source region: ${context.active ? "active" : "inactive"}`);
        lines.push(`Resolution: ${context.resolution}`);
        const types = context.types;
        if (types) {
            if (types.inferred) lines.push(`Inferred type: ${types.inferred}`);
            for (const mismatch of types.mismatches) {
                lines.push(`Expected: ${mismatch.expected}`, `Actual: ${mismatch.actual}`);
                if (mismatch.location)
                    lines.push(`  Expression: ${locationText(mismatch.location)}`);
                for (const origin of mismatch.origins)
                    if (origin.location)
                        lines.push(`  ${origin.label}: ${locationText(origin.location)}`);
            }
            for (const candidate of types.candidates) {
                lines.push(
                    `${candidate.status === "unknown" ? "Unproven candidate" : "Rejected candidate"}: ${candidate.target}`,
                    `  Signature: ${candidate.signature}`,
                    `  Reason: ${candidate.reason}`,
                );
                if (candidate.declaration)
                    lines.push(`  Declaration: ${locationText(candidate.declaration)}`);
                for (const [index, argument] of candidate.arguments.entries())
                    lines.push(
                        `  Related argument${candidate.argumentIndices?.[index] === undefined ? "" : ` ${candidate.argumentIndices[index]}`}: ${locationText(argument)}`,
                    );
            }
            for (const constraint of types.constraints) {
                lines.push(
                    `Constraint (${constraint.status === "refuted" ? "proved false" : "not established"}): ${constraint.constraint}`,
                );
                if (constraint.reason) lines.push(`  Reason: ${constraint.reason}`);
                if (constraint.assumptions.length)
                    lines.push(`  Assumptions: ${constraint.assumptions.join("; ")}`);
                if (constraint.origin) lines.push(`  Origin: ${locationText(constraint.origin)}`);
            }
        }
        const project = context.project;
        if (project) {
            lines.push(
                `Manifest: ${project.manifest}`,
                `Module: ${project.module ?? "(unnamed)"}`,
                `Direct requirements: ${project.requires.join(", ") || "(none)"}`,
                `Selected roots: ${project.roots.join(", ") || "all modules"}`,
                `Variables: ${JSON.stringify(project.variables)}`,
                `Target: ${project.target ?? "(default)"}`,
                `Include paths: ${project.includePaths.join(", ") || "(default)"}`,
            );
        }
        lines.push("Visible files:", ...context.visibleFiles.map((uri) => `  ${uri}`));
        if (context.candidates.length) {
            lines.push("Declarations with this name (byte offsets):");
            for (const candidate of context.candidates) {
                lines.push(`  ${candidate.uri}:${candidate.offset} — ${candidate.reason}`);
            }
        }
    }
    return lines.join("\n");
}

function locationText(location: SourceLocation): string {
    return `${location.uri}:${location.range.start.line + 1}:${location.range.start.character + 1}`;
}

/** Only structured file locations become links; project text stays plain text. */
export function analysisSourceLinks(
    result: AnalysisContext,
    text: string,
): { start: number; end: number; location: SourceLocation }[] {
    const links: { start: number; end: number; location: SourceLocation }[] = [];
    for (const context of result.contexts) {
        const types = context.types;
        if (!types) continue;
        const locations = [
            ...types.mismatches.flatMap((mismatch) => [
                mismatch.location,
                ...mismatch.origins.map((origin) => origin.location),
            ]),
            ...types.candidates.flatMap((candidate) => [
                candidate.declaration,
                ...candidate.arguments,
            ]),
            ...types.constraints.map((constraint) => constraint.origin),
        ];
        for (const location of locations) {
            if (!location) continue;
            try {
                if (new URL(location.uri).protocol !== "file:") continue;
            } catch {
                continue;
            }
            const label = locationText(location);
            let at = text.indexOf(label);
            while (at !== -1) {
                if (!links.some((link) => link.start === at))
                    links.push({ start: at, end: at + label.length, location });
                at = text.indexOf(label, at + label.length);
            }
        }
    }
    return links;
}

/** Retry cancelled reads only while the original document is still current.
 * Waiting for a quiet project avoids exhausting retries during a reload.
 * Explicit cancellation and source edits never revive a retained position.
 */
export async function readAnalysisView<T>(
    current: () => boolean,
    ready: () => boolean,
    request: () => Promise<T>,
    pause: () => Promise<void> = () => new Promise((resolve) => setTimeout(resolve, 25)),
): Promise<T | undefined> {
    for (let attempt = 0; attempt < 3 && current(); attempt++) {
        for (let tick = 0; tick < 40 && current() && !ready(); tick++) await pause();
        if (!current()) return undefined;
        try {
            const result = await request();
            return current() ? result : undefined;
        } catch (error) {
            const code =
                typeof error === "object" && error !== null && "code" in error
                    ? error.code
                    : undefined;
            if (code === -32800) return undefined;
            if (code !== -32801 && code !== -32802) throw error;
        }
    }
    return undefined;
}
