/**
 * Devo TUI splash branding — DEVO wordmark for BrandSplashHeader.
 *
 * Version is NOT baked into the logo. BrandSplashHeader places `v{version}` in
 * the meta column (title row) from DEVO_TUI_VERSION / host `version` option.
 */

import { readFileSync } from "node:fs";

/** DEVO block wordmark (no border). Keep rows free of version text. */
export const DEVO_COMPACT_ORBIT_LOGO = `██████╗  ███████╗██╗   ██╗ ██████╗
██╔══██╗ ██╔════╝██║   ██║██╔═══██╗
██║  ██║ █████╗  ██║   ██║██║   ██║
██║  ██║ ██╔══╝  ╚██╗ ██╔╝██║   ██║
██████╔╝ ███████╗ ╚████╔╝ ╚██████╔╝
╚═════╝  ╚══════╝  ╚═══╝   ╚═════╝`;

export const DEVO_APP_TITLE = "devo";
export const DEVO_SPLASH_TITLE = "devo";

const packageVersion: string = JSON.parse(
  readFileSync(new URL("../package.json", import.meta.url), "utf8"),
).version;

/** Match the launching CLI, or the TUI package when started directly. */
export const DEVO_TUI_VERSION = process.env.DEVO_VERSION?.trim() || packageVersion;
