import { spawn, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";

let harnessProcess: ChildProcess | null = null;

function findHarnessBinary(extensionRoot: string): string {
  if (process.env.LSP_AGENT_HARNESS_BINARY) {
    return process.env.LSP_AGENT_HARNESS_BINARY;
  }

  const candidates = [
    resolve(extensionRoot, "target/debug/harness"),
    resolve(extensionRoot, "harness/target/debug/harness"),
  ];

  const found = candidates.find((p) => existsSync(p));
  if (!found) {
    throw new Error(
      "Harness binary not found. Build with 'cargo build -p harness' or set LSP_AGENT_HARNESS_BINARY.",
    );
  }

  return found;
}

export function startHarness(extensionRoot: string): ChildProcess {
  const binaryPath = findHarnessBinary(extensionRoot);

  harnessProcess = spawn(binaryPath, [], {
    stdio: ["ignore", "ignore", "inherit"],
    env: { ...process.env, RUST_BACKTRACE: "1" },
  });

  harnessProcess.on("exit", (code) => {
    console.error(`[Pi Extension] Harness exited with code ${code}`);
  });

  return harnessProcess;
}

export function stopHarness(): void {
  harnessProcess?.kill();
  harnessProcess = null;
}
