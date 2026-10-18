import { execFile, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { closeSync, existsSync, mkdirSync, mkdtempSync, openSync, rmSync, writeFileSync } from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

const packageDir = path.resolve(fileURLToPath(new URL(".", import.meta.url)));
const repoRoot = path.resolve(packageDir, "..");
const runtimeRoot = path.join(packageDir, ".runtime");
const runtimeFile = path.join(runtimeRoot, "current.json");
const artifactDir = path.resolve(process.env.MPGS_E2E_ARTIFACT_DIR ?? path.join(packageDir, "artifacts"));
const executableSuffix = process.platform === "win32" ? ".exe" : "";
const seedServerHost = process.env.MPGS_E2E_SERVER_HOST ?? "127.0.0.1";
const seedServerPort = 18080;
const seedServerUrlHost = seedServerHost.includes(":") ? `[${seedServerHost}]` : seedServerHost;
const seedServerOrigin = `http://${seedServerUrlHost}:${seedServerPort}`;

function configuredPath(envName, fallback) {
  const configured = process.env[envName];
  return path.resolve(repoRoot, configured || fallback);
}

const application = configuredPath(
  "MPGS_E2E_APP",
  `apps/desktop/src-tauri/target/debug/lobbytally-desktop${executableSuffix}`,
);
const serverBinary = configuredPath(
  "MPGS_E2E_SERVER",
  `target/debug/mpgs-server${executableSuffix}`,
);
const dbtoolBinary = configuredPath(
  "MPGS_E2E_DBTOOL",
  `target/debug/mpgs-dbtool${executableSuffix}`,
);
const tauriDriverBinary = process.env.TAURI_DRIVER_BINARY
  ? path.resolve(process.env.TAURI_DRIVER_BINARY)
  : path.join(
      process.env.CARGO_HOME ?? path.join(os.homedir(), ".cargo"),
      "bin",
      `tauri-driver${executableSuffix}`,
    );

let serverProcess;
let serverLogFd;
let driverProcess;
let driverLogFd;
let runtimeDir;
let fixtureDatabase;
let fixtureCreated = false;

function requireFile(file, label) {
  if (!existsSync(file)) {
    throw new Error(`${label} was not found at ${file}`);
  }
}

async function requirePortAvailable(host, port) {
  await new Promise((resolve, reject) => {
    const probe = net.createServer();
    probe.unref();
    probe.once("error", (error) => {
      reject(new Error(`required E2E port ${port} is unavailable: ${error.message}`));
    });
    probe.listen({ host, port, exclusive: true }, () => {
      probe.close(resolve);
    });
  });
}

async function stopProcess(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  await new Promise((resolve, reject) => {
    const finished = (error) => {
      clearTimeout(timer);
      clearTimeout(timeout);
      child.off("exit", onExit);
      if (error) reject(error);
      else resolve();
    };
    const onExit = () => finished();
    const timer = setTimeout(() => {
      try {
        child.kill("SIGKILL");
      } catch (error) {
        finished(error?.code === "ESRCH" ? undefined : error);
      }
    }, 5_000);
    const timeout = setTimeout(() => finished(new Error("test process did not exit")), 10_000);
    child.once("exit", onExit);
    try {
      child.kill("SIGTERM");
    } catch (error) {
      finished(error?.code === "ESRCH" ? undefined : error);
    }
  });
}

async function fixtureCommand(action) {
  await execFileAsync(dbtoolBinary, ["e2e-fixture", action, fixtureDatabase], {
    cwd: repoRoot,
    env: process.env,
    windowsHide: true,
    timeout: 60_000,
    maxBuffer: 512 * 1024,
  });
}

async function cleanupFixture() {
  try {
    await stopProcess(serverProcess);
  } finally {
    if (fixtureCreated) {
      await fixtureCommand("drop");
      fixtureCreated = false;
    }
  }
}

async function waitForHttp(url, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(750) });
      if (response.ok) return;
      lastError = new Error(`HTTP ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for ${url}: ${lastError?.message ?? "unknown error"}`);
}

async function waitForProcessHttp(child, url, timeoutMs) {
  let onExit;
  const earlyExit = new Promise((_, reject) => {
    onExit = (code, signal) => {
      reject(new Error(`mpgs-server exited before readiness (code=${code}, signal=${signal})`));
    };
    child.once("exit", onExit);
  });
  try {
    await Promise.race([waitForHttp(url, timeoutMs), earlyExit]);
    await new Promise((resolve, reject) => {
      if (child.exitCode !== null) {
        reject(new Error(`mpgs-server exited after readiness (code=${child.exitCode})`));
        return;
      }
      const onStabilityExit = (code, signal) => {
        clearTimeout(timer);
        reject(new Error(`mpgs-server exited after readiness (code=${code}, signal=${signal})`));
      };
      const timer = setTimeout(() => {
        child.off("exit", onStabilityExit);
        resolve();
      }, 250);
      child.once("exit", onStabilityExit);
    });
  } finally {
    child.off("exit", onExit);
  }
}

async function waitForPort(port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const connected = await new Promise((resolve) => {
      const socket = net.createConnection({ host: "127.0.0.1", port });
      socket.setTimeout(500);
      socket.once("connect", () => {
        socket.destroy();
        resolve(true);
      });
      const failed = () => {
        socket.destroy();
        resolve(false);
      };
      socket.once("error", failed);
      socket.once("timeout", failed);
    });
    if (connected) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for tauri-driver on port ${port}`);
}

function configureIsolatedDesktopData(dataDir) {
  process.env.MPGS_CLIENT_DATA_DIR = dataDir;
  process.env.WEBVIEW2_USER_DATA_FOLDER = path.join(dataDir, "webview2");
  process.env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=0";
  if (process.platform === "win32") {
    process.env.APPDATA = path.join(dataDir, "roaming");
    process.env.LOCALAPPDATA = path.join(dataDir, "local");
  } else {
    process.env.XDG_DATA_HOME = path.join(dataDir, "data");
    process.env.XDG_CONFIG_HOME = path.join(dataDir, "config");
    process.env.XDG_CACHE_HOME = path.join(dataDir, "cache");
    process.env.TAURI_WEBVIEW_AUTOMATION = "true";
  }
}

function safeName(value) {
  return value.replace(/[^a-z0-9_-]+/gi, "-").replace(/^-|-$/g, "").slice(0, 80) || "failure";
}

export const config = {
  runner: "local",
  host: "127.0.0.1",
  port: 4444,
  specs: ["./specs/**/*.e2e.mjs"],
  maxInstances: 1,
  capabilities: [
    {
      maxInstances: 1,
      "tauri:options": { application },
    },
  ],
  logLevel: "info",
  outputDir: path.join(artifactDir, "wdio"),
  bail: 0,
  waitforTimeout: 15_000,
  connectionRetryTimeout: 120_000,
  connectionRetryCount: 2,
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: {
    ui: "bdd",
    timeout: 180_000,
  },

  onPrepare: async () => {
    requireFile(application, "Tauri application");
    requireFile(serverBinary, "mpgs-server");
    requireFile(dbtoolBinary, "mpgs-dbtool");
    requireFile(tauriDriverBinary, "tauri-driver");
    if (!process.env.MPGS_TEST_DATABASE_URL) {
      throw new Error("MPGS_TEST_DATABASE_URL must point to local PostgreSQL with database creation permission");
    }
    let fixtureUrl;
    try {
      fixtureUrl = new URL(process.env.MPGS_TEST_DATABASE_URL);
    } catch {
      throw new Error("MPGS_TEST_DATABASE_URL must be a PostgreSQL URL");
    }
    fixtureDatabase = `mpgs_e2e_${randomBytes(16).toString("hex")}`;
    fixtureUrl.pathname = `/${fixtureDatabase}`;
    await requirePortAvailable(seedServerHost, seedServerPort);
    mkdirSync(runtimeRoot, { recursive: true });
    mkdirSync(artifactDir, { recursive: true });
    runtimeDir = mkdtempSync(path.join(os.tmpdir(), "mpgs-e2e-"));
    const desktopDataDir = path.join(runtimeDir, "desktop-data");
    configureIsolatedDesktopData(desktopDataDir);

    try {
      await fixtureCommand("create");
      fixtureCreated = true;
      serverLogFd = openSync(path.join(artifactDir, "mpgs-server.log"), "w");
      serverProcess = spawn(serverBinary, [], {
        cwd: repoRoot,
        env: {
          ...process.env,
          MPGS_DATABASE_URL: fixtureUrl.toString(),
          MPGS_CRON_SECRET: randomBytes(32).toString("hex"),
          // Avatar uploads are outside this catalog/accounts/offline suite.
          MPGS_SUPABASE_URL: "https://storage.invalid",
          MPGS_SUPABASE_SERVICE_ROLE_KEY: "local-e2e-storage-unused",
          MPGS_AI_PROVIDER: "disabled",
          MPGS_AI_EMBED_PROVIDER: "hash",
          MPGS_RATE_LIMIT_ENABLED: "false",
          MPGS_BIND_ADDR: `${seedServerUrlHost}:${seedServerPort}`,
        },
        stdio: ["ignore", serverLogFd, serverLogFd],
        windowsHide: true,
      });
      writeFileSync(
        runtimeFile,
        JSON.stringify({
          runtimeDir,
          desktopDataDir,
          serverPid: serverProcess.pid,
          serverHealthUrl: `${seedServerOrigin}/health/live`,
          serverStopped: false,
          fixtureDatabase,
        }),
        "utf8",
      );
      await waitForProcessHttp(serverProcess, `${seedServerOrigin}/health/ready`, 20_000);
    } catch (error) {
      try {
        await cleanupFixture();
      } catch (cleanupError) {
        throw new AggregateError([error, cleanupError], "E2E startup and fixture cleanup failed");
      }
      throw error;
    }
  },

  beforeSession: async (_wdioConfig, capabilities) => {
    const runtime = JSON.parse(await import("node:fs/promises").then(({ readFile }) => readFile(runtimeFile, "utf8")));
    configureIsolatedDesktopData(runtime.desktopDataDir);
    const tauriOptions = capabilities["tauri:options"];
    if (!tauriOptions || typeof tauriOptions !== "object") {
      throw new Error("missing tauri:options capability");
    }
    tauriOptions.webviewOptions = {
      userDataFolder: path.join(runtime.desktopDataDir, "webview2"),
      additionalBrowserArguments: ["remote-debugging-port=0"],
    };
    driverLogFd = openSync(path.join(artifactDir, "tauri-driver.log"), "a");
    driverProcess = spawn(tauriDriverBinary, [], {
      cwd: repoRoot,
      env: process.env,
      stdio: ["ignore", driverLogFd, driverLogFd],
      windowsHide: true,
    });
    await waitForPort(4444, 15_000);
    if (driverProcess.exitCode !== null) {
      throw new Error(`tauri-driver exited early with code ${driverProcess.exitCode}`);
    }
  },

  afterTest: async (test, _context, result) => {
    if (result.passed || !globalThis.browser?.sessionId) return;
    const filename = `${Date.now()}-${safeName(test.title)}.png`;
    await globalThis.browser.saveScreenshot(path.join(artifactDir, filename));
  },

  afterSession: async () => {
    await stopProcess(driverProcess);
    if (driverLogFd !== undefined) closeSync(driverLogFd);
    driverLogFd = undefined;
  },

  onComplete: async (exitCode) => {
    await cleanupFixture();
    if (serverLogFd !== undefined) closeSync(serverLogFd);
    serverLogFd = undefined;
    if (exitCode === 0 && runtimeDir) {
      rmSync(runtimeDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
      rmSync(runtimeRoot, { recursive: true, force: true });
    }
  },
};
