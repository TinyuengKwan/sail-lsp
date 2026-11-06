import { writeFileSync } from "node:fs";
import { spawn } from "node:child_process";

const [mode, destination, encodedContext, ...args] = process.argv.slice(2);
const descendant = mode === "wait" ? spawn(process.execPath, ["-e", "setInterval(()=>{},1000)"], { stdio: "ignore" }) : undefined;
writeFileSync(destination, JSON.stringify({
    pid: process.pid,
    descendant: descendant?.pid,
    cwd: process.cwd(),
    context: JSON.parse(encodedContext === "@environment" ? process.env.SAIL_LSP_PROJECT_CONTEXT : encodedContext),
    args,
    compiler: process.env.TASK_COMPILER,
    inherited: process.env.PROJECT_TASK_TEST_EXTRA,
}));
if (mode === "wait") setInterval(() => {}, 1_000);
if (mode === "fail") process.exitCode = 7;
