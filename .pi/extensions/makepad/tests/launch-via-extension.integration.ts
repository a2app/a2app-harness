import { connectToHarness, getDocHandle } from "../doc-bridge.js";
import { registerTools } from "../tools.js";

interface ToolDef {
  name: string;
  execute: (
    id: string,
    params: Record<string, unknown>,
    signal: AbortSignal,
    onUpdate?: (update: unknown) => void,
  ) => Promise<Record<string, unknown>>;
}

class FakePi {
  private readonly tools = new Map<string, ToolDef>();

  registerTool(tool: ToolDef): void {
    this.tools.set(tool.name, tool);
  }

  getTool(name: string): ToolDef {
    const tool = this.tools.get(name);
    if (!tool) {
      throw new Error(`Tool '${name}' was not registered`);
    }
    return tool;
  }
}

async function waitFor(
  predicate: () => boolean,
  timeoutMs: number,
  label: string,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() <= deadline) {
    if (predicate()) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out waiting for ${label}`);
}

async function main(): Promise<void> {
  const timeoutMs = Number(process.env.SMOKE_TIMEOUT_MS ?? "30000");
  const appId = process.env.SMOKE_APP_ID ?? `ts-ext-smoke-${Date.now()}`;

  await connectToHarness();

  const fakePi = new FakePi();
  registerTools(fakePi as unknown as any);

  const launchTool = fakePi.getTool("launch_makepad_app");
  const closeTool = fakePi.getTool("close_makepad_app");

  const launch = await launchTool.execute(
    "integration-launch",
    {
      app_id: appId,
      standard_app: "todo",
      splash_body: "",
    },
    AbortSignal.timeout(timeoutMs),
  );

  if (launch.isError) {
    throw new Error(`Launch tool returned error: ${JSON.stringify(launch)}`);
  }

  await waitFor(
    () => {
      const doc = getDocHandle().doc();
      return (
        doc?.pending_app?.id === appId &&
        doc?.pending_app?.status === "Launched"
      );
    },
    timeoutMs,
    `app '${appId}' to be launched`,
  );

  const doc = getDocHandle().doc();
  const splash = String(doc?.pending_app?.splash_body ?? "");
  if (!splash.includes("add_todo") || !splash.includes("toggle_todo")) {
    throw new Error(
      "Todo splash body does not match extension standard app template",
    );
  }

  const close = await closeTool.execute(
    "integration-close",
    { app_id: appId },
    AbortSignal.timeout(timeoutMs),
  );

  if (close.isError) {
    throw new Error(`Close tool returned error: ${JSON.stringify(close)}`);
  }

  await waitFor(
    () => {
      const current = getDocHandle().doc();
      return current?.pending_app === null;
    },
    timeoutMs,
    `app '${appId}' to close`,
  );

  console.log(`Extension TypeScript integration succeeded for ${appId}`);
  process.exit(0);
}

main().catch((err) => {
  const message = err instanceof Error ? err.stack ?? err.message : String(err);
  console.error(message);
  process.exit(1);
});
