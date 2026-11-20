import { always, eventually, now } from "@antithesishq/bombadil";
import { CharSet } from "@antithesishq/bombadil/actions";
import { actions, extract, weighted, type ActionTemplate } from "@antithesishq/bombadil/terminal";
export { exitSuccess, noReplacementChars } from "@antithesishq/bombadil/terminal/defaults";

const screen = extract(({ grid, lastAction, exitStatus }) => {
    const rows = Array.from({ length: grid.size.rows }, (_, i) => grid.rowText(i).trimEnd());
    const after = (prefix: string) => rows.find(row => row.startsWith(prefix))?.slice(prefix.length) ?? "";
    const title = after("Page: ");
    const previous = after("Previous: ");
    const ready = Number(after("Ready #") || -1);
    const depth = Number(after("Back: ") || 0);
    const runningLine = /^(\d+): ?(.*)$/.exec(after("Running #"));
    const running = runningLine ? Number(runningLine[1]) : -1;
    const prompt = rows.includes("nt>");
    const priorBack = /^(\d+) (.+)$/.exec(after("Before back: "));
    return {
        rows, title, previous, ready, running, prompt, depth,
        runningCommand: runningLine?.[2] ?? "",
        completedCommand: after("Command: "),
        beforePrevious: priorBack?.[2] ?? "",
        beforeDepth: Number(priorBack?.[1] ?? 0),
        beforeQuery: after("Before state: "),
        command: lastAction && "TypeText" in lastAction ? lastAction.TypeText.trim() : "",
        exit: exitStatus?.code ?? null,
    };
});

const canAct = () => screen.current.prompt && screen.current.running === -1 && screen.current.exit === null;
const finished = (serial: number) => screen.current.ready >= serial && screen.current.prompt;
// Reproduction can drive the next input before observing an intermediate prompt.
// Check each sampled receipt independently, and let a newer receipt discharge the
// older timing obligation when the terminal driver sampled past it.
const resultFor = (serial: number, matches: () => boolean) =>
    finished(serial) && (screen.current.ready > serial || matches());

export const noPanic = always(() =>
    !screen.current.rows.some(row => /(^PANIC\b|^thread .* panicked at )/i.test(row))
);

export const commandReturnsToPrompt = always(() => {
    const { command, ready, running, prompt } = screen.current;
    // The PTY's echo can arrive before the REPL acknowledges reading the line.
    const echoed = !prompt && ready >= 0 && command !== "quit"
        && screen.current.rows.includes(`nt> ${command}`.trimEnd());
    const serial = running >= 0 ? running : ready + 1;
    return now(() => running >= 0 || echoed)
        .implies(eventually(() => finished(serial)).within(5, "seconds"));
});

export const backRestoresPreviousTitle = always(() => {
    const { running, previous, runningCommand, beforePrevious, depth, beforeDepth } = screen.current;
    if (running >= 0 && runningCommand === "back" && previous !== "") {
        return eventually(() => resultFor(running,
            () => screen.current.title === previous && screen.current.depth === depth - 1)).within(5, "seconds");
    }
    return now(() => !canAct() || screen.current.completedCommand !== "back"
        || beforeDepth === 0 || (screen.current.title === beforePrevious && depth === beforeDepth - 1));
});

export const searchShowsTheQuery = always(() => {
    const { running, runningCommand, completedCommand } = screen.current;
    const query = /^search\s+(.+)$/.exec(running >= 0 ? runningCommand : completedCommand)?.[1];
    if (query === undefined) return now(() => true);
    const matches = () => screen.current.rows.includes(`page=search query=${query}`);
    if (running >= 0) return eventually(() => resultFor(running, matches)).within(5, "seconds");
    return now(() => !canAct() || matches());
});

export const stateReportsTheSearchQuery = always(() => {
    const { running, runningCommand, completedCommand, title, beforeQuery } = screen.current;
    const command = running >= 0 ? runningCommand : completedCommand;
    const query = running >= 0
        ? (title.startsWith("Search / ") ? title.slice("Search / ".length) : "")
        : beforeQuery;
    if (command !== "state" || query === "") return now(() => true);
    const matches = () => screen.current.rows.includes(`State page=search query=${query}`);
    if (running >= 0) return eventually(() => resultFor(running, matches)).within(5, "seconds");
    return now(() => !canAct() || matches());
});

export const quitExits = always(() =>
    now(() => screen.current.command === "quit")
        .implies(eventually(() => screen.current.exit === 0).within(5, "seconds"))
);

function line(command: string): ActionTemplate {
    return { TypeText: { CharSet: CharSet.fromLiterals(command + "\n") } };
}

const needsState = () =>
    canAct() && /^search\s+\S/.test(screen.current.completedCommand);
const explore = () => canAct();

export const commands = weighted([
    [20_000, actions(() => needsState() ? [line("state")] : [])],
    [1, { TypeText: { Regexp: "" } }],
    [1, actions(() => explore() ? [line("quit")] : [])],
    [1_400, actions(() => explore() ? [
        line("home"), line("home music"), line("home gaming"),
        line("search rust"), line("search rust programming"), line("search rust prog"),
        line("watch 0y6RKiIk6cs"), line("watch 5C_HPTJg5ek"),
        line("channel UCrB_lbcnNsWXbvTPzpoeYWg"), line("state"),
    ] : [])],
    [2_000, actions(() => {
        if (!explore()) return [];
        const { rows, previous } = screen.current;
        const choices = rows.flatMap(row => {
            const number = /^(\d+)\. /.exec(row)?.[1];
            if (number === undefined) return [];
            return [line(/^page=(comments|replies) /.test(rows.find(row => row.startsWith("page=")) ?? "") ? `replies ${number}` : `open ${number}`)];
        });
        if (previous) choices.push(line("back"), line("back"));
        if (rows.includes("Comments available")) choices.push(line("comments"));
        if (rows.includes("More available")) choices.push(line("more"));
        if (rows.some(row => row.startsWith("page=search "))) choices.push(line("state"), line("state"));
        for (const row of rows) {
            if (row.startsWith("Channel: ")) choices.push(line("channel " + row.slice("Channel: ".length)));
        }
        return choices;
    })],
    [300, actions(() => explore() ? [{ TypeText: { Regexp: "[a-zA-Z0-9!? ]{0,35}\n" } }] : [])],
    // End a session occasionally. The runner uses independent sessions for long exploration.
    [100, actions(() => explore() && screen.current.ready >= 300 ? [line("quit")] : [])],
]);
