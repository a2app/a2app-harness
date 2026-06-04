import WebSocket from "ws";

const HARNESS_WS = "ws://127.0.0.1:2341/";
const CONNECT_TIMEOUT_MS = 15_000;

interface HarnessMessage {
  type: string;
  app_id?: string;
  status?: string;
  response?: string;
}

async function main(): Promise<void> {
  const timeoutMs = Number(process.env.SMOKE_TIMEOUT_MS ?? "30000");
  const appId = process.env.SMOKE_APP_ID ?? `ts-ext-smoke-${Date.now()}`;

  // Connect to harness JSON WS and wait for welcome message
  // Attach the message handler BEFORE resolving the connection to avoid races.
  const { ws, welcome } = await new Promise<{ ws: WebSocket; welcome: HarnessMessage }>((resolve, reject) => {
    const deadline = Date.now() + CONNECT_TIMEOUT_MS;
    const tryConnect = () => {
      if (Date.now() > deadline) {
        reject(new Error("Timed out connecting to harness"));
        return;
      }
      const socket = new WebSocket(HARNESS_WS);
      const welcomeTimeout = setTimeout(
        () => reject(new Error("Timeout waiting for welcome")),
        5000,
      );
      socket.on("message", (data: Buffer) => {
        clearTimeout(welcomeTimeout);
        const msg = JSON.parse(data.toString()) as HarnessMessage;
        resolve({ ws: socket, welcome: msg });
      });
      socket.on("open", () => {
        // Message handler already set up; welcome will arrive shortly
      });
      socket.on("error", () => {
        clearTimeout(welcomeTimeout);
        socket.close();
        setTimeout(tryConnect, 500);
      });
    };
    tryConnect();
  });

  if (welcome.type !== "welcome") {
    throw new Error(`Expected welcome, got: ${JSON.stringify(welcome)}`);
  }
  console.error("Got welcome from harness");

  // Listen for status updates
  const statusPromise = new Promise<HarnessMessage>((resolve, reject) => {
    const timeout = setTimeout(
      () => reject(new Error("Timeout waiting for status")),
      timeoutMs,
    );
    ws.on("message", (data: Buffer) => {
      const msg = JSON.parse(data.toString()) as HarnessMessage;
      if (msg.type === "status" && msg.app_id === appId) {
        clearTimeout(timeout);
        resolve(msg);
      }
    });
  });

  // Send launch
  ws.send(
    JSON.stringify({
      type: "launch",
      app_id: appId,
      splash_body: `RoundedView{
        width: Fill height: Fit
        flow: Down spacing: 10 padding: 16
        draw_bg.color: #x1e1e2e draw_bg.border_radius: 10.0
        Label{text: "Integration Test" draw_text.color: #fff}
      }`,
    }),
  );
  console.error("Sent launch request");

  // Wait for status: Launched
  const status = await statusPromise;
  if (status.status !== "Launched") {
    throw new Error(`Expected Launched status, got: ${JSON.stringify(status)}`);
  }
  console.error(`App '${appId}' launched successfully`);

  // Send clear
  ws.send(
    JSON.stringify({
      type: "clear",
      app_id: appId,
    }),
  );
  console.error("Sent clear request");

  // Give harness a moment to process
  await new Promise((resolve) => setTimeout(resolve, 1000));

  ws.close();

  console.log(`Integration test succeeded for ${appId}`);
  process.exit(0);
}

main().catch((err) => {
  const message = err instanceof Error ? err.stack ?? err.message : String(err);
  console.error(message);
  process.exit(1);
});
