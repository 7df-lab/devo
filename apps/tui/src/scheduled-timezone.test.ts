import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";

for (const timezone of ["Asia/Shanghai", "America/New_York", "UTC"]) {
  test(`Scheduled next run uses local time and exposes its zone in ${timezone}`, () => {
    const source = `
      import assert from 'node:assert/strict';
      import { stripVTControlCharacters } from 'node:util';
      import { initTheme } from '@earendil-works/pi-coding-agent/theme';
      import { ScheduledManagerComponent } from './lib/coding-agent/src/modes/interactive/components/scheduled-manager.ts';
      initTheme('dark');
      const job = { id: 'one', sessionId: 'session', status: 'active', source: 'heartbeat',
        schedule: { expression: 'every 1h' }, prompt: 'Check progress', runCount: 0, createdAt: '2026-01-01' };
      const component = new ScheduledManagerComponent({ getScheduledTasks: () => [{ job, sessionName: 'session' }],
        getRows: () => 24, onAction: async () => {}, onClose: () => {}, requestRender: () => {} });
      component.handleInput('\\r');
      for (const timestamp of ['2026-11-13T00:00:00Z', '2026-03-08T06:30:00Z', '2026-03-08T07:30:00Z']) {
        job.nextRunAt = timestamp;
        const expected = new Date(timestamp).toLocaleString(undefined, {
          year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', timeZoneName: 'short' });
        for (const width of [40, 100]) {
          const rendered = stripVTControlCharacters(component.render(width).join('\\n'));
          assert.ok(rendered.includes('Next: ' + expected), rendered + '\\nexpected: ' + expected);
        }
      }
      job.nextRunAt = 'invalid-date';
      assert.ok(stripVTControlCharacters(component.render(100).join('\\n')).includes('Next: invalid-date'));
      delete job.nextRunAt;
      assert.ok(stripVTControlCharacters(component.render(100).join('\\n')).includes('Next: —'));
    `;
    const result = spawnSync(process.execPath, ["--import", "tsx", "--input-type=module", "-e", source], {
      cwd: new URL("../", import.meta.url), encoding: "utf8",
      env: { ...process.env, TZ: timezone, LANG: "en_US.UTF-8" }, timeout: 30000,
    });
    assert.equal(result.status, 0, result.stderr || String(result.error));
  });
}
