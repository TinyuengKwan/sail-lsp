export interface AnalysisContext {
    file: string;
    projectMember: boolean | null;
    projectLoadError: string | null;
    reloadPending: boolean;
    unsavedManifestPolicy: string;
    contexts: {
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
