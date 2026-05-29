import { spawn, execSync, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";

const WS_PORT = 2341;
const DOC_ID_PORT = 2348;

let harnessProcess: ChildProcess | null = null;

function findHarnessBinary(workspaceRoot: string): string {
  if (process.env.LSP_AGENT_HARNESS_BINARY) {
    return process.env.LSP_AGENT_HARNESS_BINARY;
  }

  const candidates = [
    resolve(workspaceRoot, "target/debug/harness"),
    resolve(workspaceRoot, "harness/target/debug/harness"),
  ];

  const found = candidates.find((p) => existsSync(p));
  if (!found) {
    throw new Error(
      "Harness binary not found. Build with 'cargo build -p harness' or set LSP_AGENT_HARNESS_BINARY.",
    );
  }

  return found;
}

/**
 * Kill any process holding the harness ports (2341, 2348).
 * This prevents "Address already in use" crashes on restart.
 */
function killProcessesOnPorts(ports: number[]): void {
  for (const port of ports) {
    try {
      const stdout = execSync(`lsof -ti :${port} 2>/dev/null`, {
        encoding: "utf-8",
        timeout: 3000,
      }).trim();
      if (stdout) {
        const pids = stdout.split("\n").filter(Boolean);
        for (const pid of pids) {
          try {
            execSync(`kill -9 ${pid} 2>/dev/null`, { timeout: 2000 });
          } catch {
            // Process may already be gone
          }
        }
      }
    } catch {
      // lsof may fail if no process is on the port — that's fine
    }
  }
}

export function startHarness(workspaceRoot: string): ChildProcess {
  if (harnessProcess && harnessProcess.exitCode === null) {
    return harnessProcess;
  }

  // Kill any stale processes holding our ports before spawning.
  killProcessesOnPorts([WS_PORT, DOC_ID_PORT]);

  const binaryPath = findHarnessBinary(workspaceRoot);

  harnessProcess = spawn(binaryPath, [], {
    stdio: ["ignore", "ignore", "inherit"],
    env: { ...process.env, RUST_BACKTRACE: "1", MAKEPAD_HOST_WINDOWED: "1" },
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

  // Try SIGTERM first for graceful shutdown, then SIGKILL after timeout.
  try {
    harnessProcess.kill("SIGTERM");
    
    // Give it 3 seconds to shut down gracefully, then force kill.
    const killTimeout = setTimeout(() => {
      try {
        harnessProcess?.kill("SIGKILL");
      } catch {
        // Already dead
      }
    }, 3000);

    harnessProcess.on("exit", () => {
      clearTimeout(killTimeout);
    });
  } catch {
    // Process already gone
  }

  harnessProcess = null;
}
