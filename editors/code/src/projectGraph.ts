/** A dependency graph unfolded as a tree. Paths, not a global visited set,
 * stop cycles: a shared dependency remains visible under every requiring module. */
export interface ProjectGraphEntry {
    manifest: string;
    path: string;
    preferred: boolean;
    variables: Record<string, boolean | string>;
    roots: string[];
    modules: { name: string; active: boolean; requires: string[]; files: string[] }[];
}

export type GraphNode =
    | { kind: "project"; project: ProjectGraphEntry }
    | { kind: "module"; project: ProjectGraphEntry; name: string; ancestors: string[] }
    | { kind: "file"; uri: string };

export function isCycle(node: GraphNode): boolean {
    return node.kind === "module" && node.ancestors.includes(node.name);
}

export function graphChildren(node: GraphNode): GraphNode[] {
    if (node.kind === "file" || isCycle(node)) return [];
    if (node.kind === "project") {
        return node.project.modules.map((module) => ({
            kind: "module",
            project: node.project,
            name: module.name,
            ancestors: [],
        }));
    }
    const module = node.project.modules.find((module) => module.name === node.name);
    if (!module) return [];
    return [
        ...module.requires.map(
            (name): GraphNode => ({
                kind: "module",
                project: node.project,
                name,
                ancestors: [...node.ancestors, node.name],
            }),
        ),
        ...module.files.map((uri): GraphNode => ({ kind: "file", uri })),
    ];
}
