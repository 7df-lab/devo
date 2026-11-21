import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { once } from "node:events";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { test, type TestContext } from "node:test";
import { promisify } from "node:util";
import { configureFetchProxy, getProxySplashValues } from "./fetch-proxy.js";

const runNode = promisify(execFile);
const proxyModule = new URL("./fetch-proxy.ts", import.meta.url).href;

async function listen(t: TestContext, server: Server): Promise<string> {
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  t.after(() => server.close());
  return `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
}

async function runFetches(env: Record<string, string>, urls: string[]): Promise<string[]> {
  // Run the real Node global fetch without changing the test runner's environment or dispatcher.
  const { stdout } = await runNode(process.execPath, ["--import", "tsx", "--input-type=module", "-e", `
    import { configureFetchProxy } from ${JSON.stringify(proxyModule)};
    const [env, ...urls] = process.argv.slice(1);
    configureFetchProxy(JSON.parse(env));
    for (const url of urls) {
      try {
        const response = await fetch(url, { signal: AbortSignal.timeout(3000) });
        console.log(await response.text());
      } catch {
        console.log("fetch failed");
      }
    }
  `, JSON.stringify(env), ...urls], { timeout: 12_000 });
  return stdout.trim().split(/\r?\n/);
}

test("TUI Node fetch honors uppercase and lowercase proxies and NO_PROXY", async (t) => {
  const requests: string[] = [];
  const proxy = (label: string) => createServer().on("connect", (request, socket) => {
    requests.push(`${label} ${request.url}`);
    socket.end("HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
  });
  const upper = await listen(t, proxy("upper"));
  const lower = await listen(t, proxy("lower"));
  const direct = await listen(t, createServer((_request, response) => response.end("direct")));

  assert.deepEqual(await runFetches({ HTTP_PROXY: upper, HTTPS_PROXY: upper }, [
    "http://plain.invalid/", "https://secure.invalid/",
  ]), ["fetch failed", "fetch failed"]);
  assert.deepEqual(await runFetches({ HTTP_PROXY: upper, HTTPS_PROXY: upper, http_proxy: lower, https_proxy: lower, NO_PROXY: "127.0.0.1" }, [
    "http://plain.invalid/", "https://secure.invalid/", direct,
  ]), ["fetch failed", "fetch failed", "direct"]);
  assert.deepEqual(requests, ["upper plain.invalid:80", "upper secure.invalid:443", "lower plain.invalid:80", "lower secure.invalid:443"]);
});

test("proxy splash values use proxy precedence and omit credentials", () => {
  assert.deepEqual(getProxySplashValues({
    http_proxy: " http://lower-user:lower-password@proxy.example:8443/path ",
    HTTP_PROXY: "http://upper-user:upper-password@other.example:9999",
    HTTPS_PROXY: "http://https-user:https-password@https.example:9443",
  }), ["http://proxy.example:8443", "http://https.example:9443"]);
  assert.deepEqual(getProxySplashValues({}), []);
});

test("invalid proxy configuration reports no proxy credentials", () => {
  assert.throws(() => configureFetchProxy({ HTTPS_PROXY: "not a URL with fixture-password" }), {
    message: "Invalid proxy configuration. Check http_proxy, https_proxy, HTTP_PROXY, and HTTPS_PROXY.",
  });
});
