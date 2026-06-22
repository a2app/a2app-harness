import { spawn, execSync, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const JSON_WS_PORT = 2341;

let harnessProcess: ChildProcess | null = null;

function findHarnessBinary(workspaceRoot: string): string {
  if (process.env.LSP_AGENT_HARNESS_BINARY) {
    return process.env.LSP_AGENT_HARNESS_BINARY;
  }

  const candidates = [
    resolve(workspaceRoot, "target/debug/harness"),
    resolve(workspaceRoot, "harness/target/debug/harness"),
  ];

  // Also resolve from this extension file's own location (works even if CWD doesn't match project root)
  const __filename = fileURLToPath(import.meta.url);
  const extDir = dirname(__filename); // .pi/extensions/makepad/dist/
  const projectRoot = resolve(extDir, "../../../../");
  candidates.push(resolve(projectRoot, "target/debug/harness"));
  candidates.push(resolve(projectRoot, "harness/target/debug/harness"));

  const found = candidates.find((p) => existsSync(p));
  if (!found) {
    throw new Error(
      `Harness binary not found. Build with 'cargo build -p harness' or set LSP_AGENT_HARNESS_BINARY.` +
      ` (searched: ${candidates.join(", ")})`,
    );
  }

  return found;
}

function killProcessesOnPort(port: number): void {
  const ownPid = process.pid;
  try {
    const stdout = execSync(`lsof -ti :${port} -sTCP:LISTEN 2>/dev/null`, {
      encoding: "utf-8",
      timeout: 3000,
    }).trim();
    if (stdout) {
      const pids = stdout.split("\n").filter(Boolean);
      for (const pid of pids) {
        const pidNum = parseInt(pid, 10);
        if (pidNum === ownPid) continue;
        try {
          execSync(`kill -9 ${pid} 2>/dev/null`, { timeout: 2000 });
        } catch {
          // already gone
        }
      }
    }
  } catch {
    // no process on port
  }
}

export function startHarness(workspaceRoot: string): ChildProcess {
  if (harnessProcess && harnessProcess.exitCode === null) {
    return harnessProcess;
  }

  // Kill any stale process on our port
  killProcessesOnPort(JSON_WS_PORT);
  killProcessesOnPort(2342); // also clean up samod WS port

  const binaryPath = findHarnessBinary(workspaceRoot);

  harnessProcess = spawn(binaryPath, [], {
    stdio: ["ignore", "ignore", "inherit"],
    env: {
      ...process.env,
      RUST_BACKTRACE: "1",
    },
  });

  harnessProcess.on("exit", (code) => {
    console.error(`[Pi Extension] Harness exited with code ${code}`);
    harnessProcess = null;
  });

  return harnessProcess;
}

export function stopHarness(): void {
  if (!harnessProcess || harnessProcess.exitCode !== null) {
    harnessProcess = null;
    return;
  }

  try {
    harnessProcess.kill("SIGTERM");

    const killTimeout = setTimeout(() => {
      try {
        harnessProcess?.kill("SIGKILL");
      } catch {
        // already dead
      }
    }, 3000);

    harnessProcess.on("exit", () => {
      clearTimeout(killTimeout);
    });
  } catch {
    // already gone
  }

  harnessProcess = null;
}
