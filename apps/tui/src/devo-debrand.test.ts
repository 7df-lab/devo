/**
 * Devo TUI must not advertise upstream vendor brand in quit copy, update checks, or slash descriptions.
 */
import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { APP_NAME, APP_TITLE } from "../lib/coding-agent/src/config.js";
import { BUILTIN_SLASH_COMMANDS } from "../lib/coding-agent/src/core/slash-commands.js";
import {
  checkForNewPiVersion,
  getLatestPiRelease,
} from "../lib/coding-agent/src/utils/version-check.js";
import { DEVO_EXCLUDED_BUILTIN_COMMANDS } from "./host.js";
import { STRIP_VENDOR_COMMANDS } from "./native-agent-connection.js";
import { oauthErrorHtml, oauthSuccessHtml } from "../lib/ai/src/utils/oauth/oauth-page.js";
import { withOpenCodeHeaders } from "../lib/ai/src/providers/opencode-headers.js";
import { shouldRunDefaultProviderLoginFallback } from "../lib/coding-agent/src/modes/interactive/onboarding.js";
import { getPiUserAgent } from "../lib/coding-agent/src/utils/pi-user-agent.js";
import { getAvailableThemes } from "../lib/coding-agent/src/modes/interactive/theme/theme.js";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "../../..");

test("product name is devo so /quit is Quit devo", () => {
  assert.equal(APP_NAME, "devo");
  assert.equal(APP_TITLE, "devo");
  assert.deepEqual(
    BUILTIN_SLASH_COMMANDS.find((command) => command.name === "quit"),
    { name: "quit", description: "Quit devo" },
  );
});

test("slash command descriptions do not mention prime", () => {
  for (const command of BUILTIN_SLASH_COMMANDS) {
    assert.doesNotMatch(command.description, /prime/i, `/${command.name}: ${command.description}`);
  }
});

test("upstream update manifests are not consulted", async () => {
  assert.equal(await checkForNewPiVersion("0.0.0"), undefined);
  assert.equal(await getLatestPiRelease("0.0.0"), undefined);
});

test("host hides /update builtin and strips /changelog and /logs", () => {
  assert.deepEqual([...DEVO_EXCLUDED_BUILTIN_COMMANDS], ["traces", "update"]);
  assert.equal(STRIP_VENDOR_COMMANDS.has("/update"), true);
  assert.equal(STRIP_VENDOR_COMMANDS.has("/changelog"), true);
  assert.equal(STRIP_VENDOR_COMMANDS.has("/logs"), true);
});

test("first-run onboarding skips a provider hidden by the host", () => {
  assert.equal(
    shouldRunDefaultProviderLoginFallback(0, "prime-inference", ["prime-inference"]),
    false,
  );
  assert.equal(shouldRunDefaultProviderLoginFallback(0, "anthropic"), true);
  assert.equal(shouldRunDefaultProviderLoginFallback(1, "anthropic"), false);
});

test("coding-agent piConfig.name is devo", () => {
  const pkg = JSON.parse(
    readFileSync(join(repoRoot, "apps/tui/lib/coding-agent/package.json"), "utf8"),
  ) as { piConfig?: { name?: string; configDir?: string } };
  assert.deepEqual(pkg.piConfig, { name: "devo", configDir: ".devo" });
});

test("self-update fallback points to the Devo release page", () => {
  const config = readFileSync(join(repoRoot, "apps/tui/lib/coding-agent/src/config.ts"), "utf8");
  assert.match(config, /github\.com\/wangtsiao\/devo\/releases\/latest/);
  assert.doesNotMatch(config, /PrimeIntellect-ai\/prime-agent/);
});

test("builtin reference theme no longer carries the old vendor name", () => {
  const theme = JSON.parse(
    readFileSync(
      join(repoRoot, "apps/tui/lib/coding-agent/src/modes/interactive/theme/reference.json"),
      "utf8",
    ),
  ) as { name?: string };
  assert.equal(theme.name, "reference");
});

test("legacy theme setting remains hidden from the Devo theme picker", () => {
  assert.deepEqual(getAvailableThemes(), ["dark", "light"]);
});

test("TUI package repository metadata points to the Devo monorepo", () => {
  const packages = [
    ["apps/tui/lib/agent/package.json", "apps/tui/lib/agent"],
    ["apps/tui/lib/ai/package.json", "apps/tui/lib/ai"],
    ["apps/tui/lib/coding-agent/package.json", "apps/tui/lib/coding-agent"],
    ["apps/tui/lib/tui/package.json", "apps/tui/lib/tui"],
  ] as const;
  for (const [path, directory] of packages) {
    const pkg = JSON.parse(readFileSync(join(repoRoot, path), "utf8")) as {
      repository?: { url?: string; directory?: string };
    };
    assert.equal(pkg.repository?.url, "git+https://github.com/wangtsiao/devo.git", path);
    assert.equal(pkg.repository?.directory, directory, path);
  }
});

test("workspace package readmes use Devo branding", () => {
  const readmes = [
    "apps/tui/lib/agent/README.md",
    "apps/tui/lib/ai/README.md",
    "apps/tui/lib/tui/README.md",
  ];
  for (const path of readmes) {
    const text = readFileSync(join(repoRoot, path), "utf8");
    assert.doesNotMatch(text, /Prime Agent|Prime Intellect|primeintellect\.ai|prime-butterfly/i, path);
  }
});

test("Devo release notes avoid upstream product branding", () => {
  const changelog = readFileSync(join(repoRoot, "apps/tui/CHANGELOG.md"), "utf8");
  assert.doesNotMatch(changelog, /Prime Agent|Prime Intellect|Prime Inference|prime-butterfly/i);
});

test("Devo hides the upstream provider from login surfaces", () => {
  const host = readFileSync(join(repoRoot, "apps/tui/src/host.ts"), "utf8");
  assert.match(host, /excludedLoginProviderIds:\s*\["prime-inference"\]/);
});

test("hosted inference surfaces use neutral labels and keep the service endpoint", () => {
  const catalog = JSON.parse(
    readFileSync(join(repoRoot, "crates/core/providers.json"), "utf8"),
  ) as {
    provider: Record<string, { name: string; description: string; base_url: string; models: Record<string, { channel?: string }> }>;
  };
  const hosted = catalog.provider["prime-inference"];
  assert.ok(hosted);
  assert.equal(hosted.name, "Hosted Inference");
  assert.equal(hosted.description, "Hosted inference models");
  assert.equal(hosted.base_url, "https://api.pinference.ai/api/v1");
  assert.ok(Object.values(hosted.models).every((model) => model.channel === "Hosted Inference"));

  const visibleSources = [
    "apps/tui/lib/coding-agent/src/modes/interactive/components/login-dialog.ts",
    "apps/tui/lib/coding-agent/src/modes/interactive/components/prime-team-selector.ts",
    "apps/tui/lib/coding-agent/src/modes/interactive/auth-flows.ts",
    "apps/tui/lib/coding-agent/src/modes/interactive/interactive-mode.ts",
    "apps/tui/lib/ai/src/prime-inference-model-catalog.ts",
  ];
  for (const path of visibleSources) {
    const text = readFileSync(join(repoRoot, path), "utf8");
    assert.doesNotMatch(text, /Prime (Agent|Intellect|Inference|API|Team)|prime-butterfly/i, path);
  }
});

test("Devo-specific provider errors use neutral wording", () => {
  const resolver = readFileSync(
    join(repoRoot, "apps/tui/lib/coding-agent/src/core/model-resolver.ts"),
    "utf8",
  );
  const authShim = readFileSync(
    join(repoRoot, "apps/tui/lib/coding-agent/src/core/prime-inference-auth.ts"),
    "utf8",
  );
  assert.doesNotMatch(resolver, /current Prime team/i);
  assert.doesNotMatch(authShim, /Prime Inference login is not available in Devo/i);
});

test("browser OAuth callback pages use Devo branding", () => {
  for (const html of [oauthSuccessHtml("Connected"), oauthErrorHtml("Login failed")]) {
    assert.match(html, /<div class="wordmark" aria-label="Devo">DEVO<\/div>/);
    assert.doesNotMatch(html, /Prime Intellect|Prime butterfly/i);
  }
});

test("outbound agent identity uses Devo rather than the upstream product name", () => {
  assert.match(getPiUserAgent("1.2.3"), /^devo\/1\.2\.3\b/);
  const headers = withOpenCodeHeaders("opencode", "session", { authorization: "test" });
  assert.equal(headers["User-Agent"], "devo");
});

test("Herdr pane reports use the Devo agent label", () => {
  const extension = readFileSync(
    join(repoRoot, "apps/tui/lib/coding-agent/src/core/extensions/builtin/herdr-agent-state.ts"),
    "utf8",
  );
  assert.match(extension, /const agentLabel = "devo";/);
  assert.doesNotMatch(extension, /const agentLabel = "prime-agent";/);
});
