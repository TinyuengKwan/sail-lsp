// Hover actions, rust-analyzer's `experimental.hoverActions`: the server
// adds `actions: CommandLinkGroup[]` to a hover, and the client renders
// each command as a Markdown link under the popup. This is the part that
// needs no editor; client.ts wraps the text in a trusted MarkdownString.

import type * as lc from "vscode-languageclient/node";

/** `hover_actions::CommandLink`: LSP's `Command` with the tooltip VS Code renders. */
export interface CommandLink extends lc.Command {
    tooltip?: string;
}

/** `hover_actions::CommandLinkGroup`. */
export interface CommandLinkGroup {
    title?: string;
    commands: CommandLink[];
}

/** What the server adds to `lsp_types::Hover`. */
export interface HoverWithActions extends lc.Hover {
    actions?: CommandLinkGroup[];
}

/**
 * The Markdown for the action links, and the commands they stand for.
 *
 * A link runs `proxyCommand` with the command's index rather than the
 * command itself: the locations a `showReferences` carries are far too
 * long for a command URI. Groups are separated by ` | `, a titled group
 * leads with its title.
 */
export function renderHoverActions(
    groups: CommandLinkGroup[],
    proxyCommand: string,
): { markdown: string; commands: CommandLink[] } {
    const commands: CommandLink[] = [];
    const render = (command: CommandLink): string => {
        commands.push(command);
        return `[${command.title}](command:${proxyCommand}?${commands.length - 1} '${command.tooltip ?? ""}')`;
    };
    const markdown = groups
        .map(
            (group) =>
                (group.title ? group.title + " " : "") + group.commands.map(render).join(" | "),
        )
        .join(" | ");
    return { markdown, commands };
}
