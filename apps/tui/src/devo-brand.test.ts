/**
 * Smoke: BrandSplashHeader renders Devo title + DEVO wordmark (not prime agent).
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { BrandSplashHeader } from "@earendil-works/pi-coding-agent/interactive";
import { initTheme } from "@earendil-works/pi-coding-agent/theme";
import {
  DEVO_COMPACT_ORBIT_LOGO,
  DEVO_SPLASH_TITLE,
  DEVO_TUI_VERSION,
} from "./devo-brand.js";

test("Devo splash header uses DEVO wordmark logo and title", () => {
  initTheme("dark", true);
  const header = new BrandSplashHeader(DEVO_TUI_VERSION, () => "/tmp/devo", undefined, {
    logo: DEVO_COMPACT_ORBIT_LOGO,
    title: DEVO_SPLASH_TITLE,
    topPadding: true,
    getModelId: () => "deepseek-v4-flash",
  });
  // Wide enough that BrandSplashHeader keeps the wordmark beside meta.
  const lines = header.render(100).join("\n");
  assert.match(lines, /devo/);
  assert.doesNotMatch(lines, /prime agent/);
  assert.doesNotMatch(lines, /▗▄▄█▀/); // prime butterfly top row
  assert.match(lines, /██████╗/);
  assert.ok(lines.includes(`v${DEVO_TUI_VERSION}`));
  assert.match(lines, /deepseek-v4-flash/);
});

test("default splash header uses the Devo wordmark", () => {
  initTheme("dark", true);
  const header = new BrandSplashHeader(DEVO_TUI_VERSION, () => "/tmp/devo", undefined, { topPadding: true });
  const lines = header.render(100).join("\n");
  assert.match(lines, /██████╗/);
  assert.doesNotMatch(lines, /prime agent|▗▄▄█▀/i);
});

test("default dark theme uses the muted reference palette, not bright blue", () => {
  const themeDir = join(
    dirname(fileURLToPath(import.meta.url)),
    "../lib/coding-agent/src/modes/interactive/theme",
  );
  const dark = JSON.parse(readFileSync(join(themeDir, "dark.json"), "utf8")) as {
    name: string;
    vars: Record<string, string>;
    colors: Record<string, string>;
  };
  const reference = JSON.parse(readFileSync(join(themeDir, "reference.json"), "utf8")) as {
    vars: Record<string, string>;
    colors: Record<string, string>;
  };
  assert.equal(dark.name, "dark");
  assert.deepEqual(dark.vars, reference.vars);
  assert.deepEqual(dark.colors, reference.colors);
  assert.equal(dark.vars.primary, "#7c6faf");
  assert.doesNotMatch(JSON.stringify(dark), /#5f87ff|#00d7ff/);
});
