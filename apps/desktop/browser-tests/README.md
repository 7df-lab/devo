# Desktop component browser regressions

Run `bun install --frozen-lockfile` in `apps/desktop`, then `npm ci`,
`npx playwright install --with-deps chromium`, and `npm test` here.

The fixture imports the production InboxRunRow, Base UI context menu and renderer
CSS. Real Chromium handles keyboard input, focus, menu positioning, responsive
layout and successful Clipboard API use. Only run data/callbacks are controlled;
rejected/unavailable clipboard cases deliberately inject those failures.
This is component browser verification, not full Electron or backend E2E.

The standalone GitHub workflow uploads traces on failure and layout screenshots.
Do not interpret test enumeration or fixture build success as browser test success.

Search picker scenarios mount the production shared popover/search/list components
and the real ModelPickerList with controlled catalog data. They cover clear via
Enter/Space, selection, close/reopen, and 360/1000px popup geometry. The branch
fixture uses the shared composition, not Electron Git IPC or branch mutation.

Automation label scenarios mount the production InboxRunList (including
AutomationRow), AutomationDetail and CreateAutomationDialog with real Jotai atoms
and UI primitives at 360/1000px. Controlled Windows/trailing-slash paths cover all
five display sites, discovered option secondary paths, add/remove and exact save
payloads. A test-only Vite resolver replaces only automation imports of router,
backend, provider hooks and model toolbar boundaries; no native picker, network,
model execution or actual automation is invoked. This does not validate native
filesystem identity or full Electron integration. Screenshots cover the rendered
inbox/detail labels; browser assertions cover dialog labels and interactions.
