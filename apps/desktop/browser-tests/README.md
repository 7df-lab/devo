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
