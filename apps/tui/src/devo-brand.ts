/**
 * Devo TUI splash branding — DEVO wordmark for BrandSplashHeader.
 *
 * Version is NOT baked into the logo. BrandSplashHeader places `v{version}` in
 * the meta column (title row) from DEVO_TUI_VERSION / host `version` option.
 */

/** DEVO block wordmark (no border). Keep rows free of version text. */
export const DEVO_COMPACT_ORBIT_LOGO = `██████╗  ███████╗██╗   ██╗ ██████╗
██╔══██╗ ██╔════╝██║   ██║██╔═══██╗
██║  ██║ █████╗  ██║   ██║██║   ██║
██║  ██║ ██╔══╝  ╚██╗ ██╔╝██║   ██║
██████╔╝ ███████╗ ╚████╔╝ ╚██████╔╝
╚═════╝  ╚══════╝  ╚═══╝   ╚═════╝`;

export const DEVO_APP_TITLE = "devo";
export const DEVO_SPLASH_TITLE = "devo";

/** Product version shown as `devo v…` beside the wordmark. Keep in sync with workspace Cargo.toml. */
export const DEVO_TUI_VERSION = "0.1.39";
