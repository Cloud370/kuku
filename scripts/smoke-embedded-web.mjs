import { spawn } from "node:child_process";
import {
  chmod,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { createServer } from "node:http";
import { createServer as createTcpServer } from "node:net";
import { tmpdir } from "node:os";
import { isAbsolute, join, resolve, sep } from "node:path";

function parseArguments(argv) {
  const options = { binary: null, timeoutMs: 15_000 };
  for (let index = 0; index < argv.length; index += 2) {
    const name = argv[index];
    const value = argv[index + 1];
    if (name === "--binary" && value !== undefined)
      options.binary = resolve(value);
    else if (name === "--timeout-ms" && value !== undefined)
      options.timeoutMs = Number(value);
    else throw new Error(`unknown or incomplete option: ${name}`);
  }
  if (options.binary === null) throw new Error("--binary is required");
  if (!Number.isSafeInteger(options.timeoutMs) || options.timeoutMs < 1) {
    throw new Error("--timeout-ms must be a positive safe integer");
  }
  const targetSegment = `${sep}target${sep}`;
  if (options.binary.includes(targetSegment)) {
    throw new Error(
      "--binary must point to an executable extracted from a package archive",
    );
  }
  return options;
}

async function unusedPort() {
  return new Promise((resolvePort, reject) => {
    const server = createTcpServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (address === null || typeof address === "string") {
        reject(new Error("could not allocate a loopback port"));
        return;
      }
      server.close((error) =>
        error === undefined ? resolvePort(address.port) : reject(error),
      );
    });
  });
}

async function startProvider() {
  const server = createServer((request, response) => {
    if (request.method !== "POST" || request.url !== "/v1/messages") {
      response.writeHead(404).end();
      return;
    }
    let body = "";
    request.setEncoding("utf8");
    request.on("data", (chunk) => {
      body += chunk;
    });
    request.on("end", () => {
      JSON.parse(body);
      response.writeHead(200, {
        "content-type": "text/event-stream",
        connection: "close",
      });
      response.end(
        [
          "event: message_start",
          'data: {"type":"message_start","message":{"id":"msg_smoke","type":"message","role":"assistant","content":[],"model":"fixture-model","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":0}}}',
          "",
          "event: message_delta",
          'data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":1}}',
          "",
          "event: message_stop",
          'data: {"type":"message_stop"}',
          "",
        ].join("\n"),
      );
    });
  });
  await new Promise((resolveListen, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  if (address === null || typeof address === "string")
    throw new Error("provider did not bind");
  return {
    origin: `http://127.0.0.1:${address.port}`,
    stop: () =>
      new Promise((resolveClose, reject) =>
        server.close((error) =>
          error === undefined ? resolveClose() : reject(error),
        ),
      ),
  };
}

async function retryFetch(url, timeoutMs, init) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  let lastStatus;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url, init);
      if (response.ok) return response;
      lastStatus = response.status;
      await response.body?.cancel();
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
  }
  throw (
    lastError ??
    new Error(
      `timed out fetching ${url}${lastStatus === undefined ? "" : `; last status ${lastStatus}`}`,
    )
  );
}

async function api(origin, credential, method, path, body) {
  const response = await fetch(`${origin}/api/v1${path}`, {
    method,
    headers: {
      Accept: "application/json",
      Authorization: `Bearer ${credential}`,
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  if (!response.ok)
    throw new Error(
      `${method} ${path} returned ${response.status}: ${await response.text()}`,
    );
  return response.status === 204 ? null : response.json();
}

async function initialize(origin, credential, providerOrigin) {
  const status = await api(origin, credential, "GET", "/status");
  if (status.init.phase !== "required")
    throw new Error("fresh package smoke home was not unconfigured");
  let init = await api(origin, credential, "POST", "/init/providers", {
    expected_revision: status.init.server_revision,
    providers: [
      {
        provider_id: "smoke-provider",
        format: "anthropic",
        base_url: providerOrigin,
        credential: { source: "direct_value", value: "smoke-key" },
      },
    ],
    tiers: [
      {
        tier_id: "tier:smoke:balanced",
        provider_id: "smoke-provider",
        model: "fixture-model",
        purpose: "Package smoke",
        think: null,
      },
    ],
  });
  init = await api(origin, credential, "POST", "/init/default-tier", {
    expected_revision: init.server_revision,
    tier_id: "tier:smoke:balanced",
  });
  const roots = await api(origin, credential, "GET", "/registration-roots");
  const root = roots.items[0];
  if (root === undefined)
    throw new Error("package exposed no registration root");
  init = await api(origin, credential, "POST", "/init/workspace", {
    workspace: {
      expected_revision: init.server_revision,
      root_id: root.root_id,
      relative_path: "workspace",
      label: "Package smoke workspace",
    },
  });
  await api(origin, credential, "POST", "/init/test", {
    expected_revision: init.server_revision,
    tier_id: "tier:smoke:balanced",
  });
  init = await api(origin, credential, "GET", "/init/status");
  await api(origin, credential, "POST", "/init/complete", {
    expected_revision: init.server_revision,
  });
  const workspaces = await api(origin, credential, "GET", "/workspaces");
  const workspace = workspaces.items[0];
  if (workspace === undefined)
    throw new Error("initialized package exposed no workspace");
  const created = await api(origin, credential, "POST", "/tasks", {
    idempotency_key: "package-smoke-create",
    workspace_id: workspace.workspace_id,
  });
  return created.projection.task.task_id;
}

async function stopChild(child) {
  if (child.exitCode !== null) return;
  const exited = new Promise((resolveExit) => child.once("exit", resolveExit));
  child.kill("SIGTERM");
  await Promise.race([
    exited,
    new Promise((resolveTimeout) => setTimeout(resolveTimeout, 5_000)),
  ]);
  if (child.exitCode === null) child.kill("SIGKILL");
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const executable = await lstat(options.binary).catch(() => null);
  if (executable === null || !executable.isFile())
    throw new Error("extracted binary does not exist");
  const scratch = await mkdtemp(join(tmpdir(), "kuku-package-smoke-"));
  const home = join(scratch, "home");
  const registrationRoot = join(scratch, "registration-root");
  await mkdir(join(registrationRoot, "workspace"), { recursive: true });
  await mkdir(home, { recursive: true });
  const credential =
    "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
  const credentialFile = join(home, "credential");
  await writeFile(credentialFile, `${credential}\n`, { mode: 0o600 });
  await chmod(credentialFile, 0o600);
  const port = await unusedPort();
  const origin = `http://127.0.0.1:${port}`;
  const provider = await startProvider();
  const logs = [];
  const child = spawn(
    options.binary,
    [
      "web",
      "--listen",
      `127.0.0.1:${port}`,
      "--auth-token-file",
      credentialFile,
      "--registration-root",
      `Package=${registrationRoot}`,
    ],
    {
      env: {
        ...process.env,
        KUKU_HOME: home,
        NO_PROXY: "127.0.0.1,localhost",
        no_proxy: "127.0.0.1,localhost",
      },
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    },
  );
  child.stdout.on("data", (chunk) => logs.push(chunk.toString()));
  child.stderr.on("data", (chunk) => logs.push(chunk.toString()));
  try {
    const health = await retryFetch(`${origin}/health`, options.timeoutMs);
    if (!health.ok) throw new Error(`health returned ${health.status}`);
    const htmlResponse = await retryFetch(`${origin}/`, options.timeoutMs);
    const html = await htmlResponse.text();
    if (!htmlResponse.ok) {
      throw new Error(
        `embedded HTML returned ${htmlResponse.status}: ${html.slice(0, 200)}`,
      );
    }
    const asset = html.match(/\/assets\/[^"']+\.js/)?.[0];
    if (asset === undefined)
      throw new Error("embedded HTML has no hashed JavaScript asset");
    const assetResponse = await retryFetch(
      `${origin}${asset}`,
      options.timeoutMs,
    );
    if (
      !assetResponse.ok ||
      !assetResponse.headers.get("content-type")?.includes("javascript")
    ) {
      throw new Error("embedded JavaScript asset has an invalid response");
    }
    const unauthenticated = await fetch(`${origin}/api/v1/status`);
    if (unauthenticated.status !== 401)
      throw new Error("status did not require authentication");
    const taskId = await initialize(origin, credential, provider.origin);
    const deepLink = await fetch(
      `${origin}/tasks/${encodeURIComponent(taskId)}`,
    );
    if (
      !deepLink.ok ||
      !(await deepLink.text()).includes('<div id="root"></div>')
    ) {
      throw new Error("Task deep link did not use the embedded SPA fallback");
    }
    console.log(
      `embedded package smoke passed: ${isAbsolute(options.binary) ? options.binary : resolve(options.binary)}`,
    );
  } catch (error) {
    process.stderr.write(logs.join(""));
    throw error;
  } finally {
    await stopChild(child);
    await provider.stop();
    await rm(scratch, { force: true, recursive: true });
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
