import { EnvHttpProxyAgent, setGlobalDispatcher } from "undici";

export interface FetchProxyEnvironment {
  httpProxy: string;
  httpsProxy: string;
}

/** Resolve proxy environment values with the same precedence used by Node fetch setup. */
export function resolveFetchProxyEnvironment(env: NodeJS.ProcessEnv): FetchProxyEnvironment {
  return {
    httpProxy: env.http_proxy?.trim() || env.HTTP_PROXY?.trim() || "",
    httpsProxy: env.https_proxy?.trim() || env.HTTPS_PROXY?.trim() || "",
  };
}

/** Return safe proxy destinations for the startup splash without URL credentials. */
export function getProxySplashValues(env: NodeJS.ProcessEnv): string[] {
  const { httpProxy, httpsProxy } = resolveFetchProxyEnvironment(env);
  return [...new Set([httpProxy, httpsProxy].filter(Boolean).map((proxy) => {
    try {
      const url = new URL(proxy);
      return `${url.protocol}//${url.hostname}${url.port ? `:${url.port}` : ""}`;
    } catch {
      // Invalid values are rejected by configureFetchProxy before the TUI starts.
      return "";
    }
  }).filter(Boolean))];
}

/** Node's fetch on the minimum supported Node version does not read proxy environment variables. */
export function configureFetchProxy(env: NodeJS.ProcessEnv): void {
  const { httpProxy, httpsProxy } = resolveFetchProxyEnvironment(env);
  if (!httpProxy && !httpsProxy) return;

  const noProxy = env.no_proxy?.trim() || env.NO_PROXY?.trim() || "";
  try {
    setGlobalDispatcher(new EnvHttpProxyAgent({ httpProxy, httpsProxy, noProxy }));
  } catch {
    // A proxy URL can contain credentials. Never echo it or fall back to a direct connection.
    throw new Error("Invalid proxy configuration. Check http_proxy, https_proxy, HTTP_PROXY, and HTTPS_PROXY.");
  }
}
