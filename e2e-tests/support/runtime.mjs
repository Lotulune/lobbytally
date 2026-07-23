import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const packageDir = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
export const runtimeFile = path.join(packageDir, ".runtime", "current.json");

export async function readRuntime() {
  return JSON.parse(await readFile(runtimeFile, "utf8"));
}

export async function stopSeedServer() {
  const runtime = await readRuntime();
  if (runtime.serverStopped) return;
  const healthUrl = runtime.serverHealthUrl ?? "http://127.0.0.1:17880/health/live";

  try {
    process.kill(runtime.serverPid, "SIGTERM");
  } catch (error) {
    if (error?.code !== "ESRCH") throw error;
  }

  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    try {
      await fetch(healthUrl, {
        signal: AbortSignal.timeout(500),
      });
    } catch {
      await writeFile(runtimeFile, JSON.stringify({ ...runtime, serverStopped: true }), "utf8");
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("seed server did not stop within 10 seconds");
}
