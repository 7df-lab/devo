/** Startup-only selector. Admission and persisted trust remain in WorkspaceTrust. */
import type { Readable } from "node:stream";
import { matchesKey, ProcessTerminal, TUI, wrapTextWithAnsi, type Component, type Terminal } from "@earendil-works/pi-tui";

export function promptWorkspaceTrust(cwd: string): Promise<boolean> {
  if (!process.stdin.isTTY || !process.stdout.isTTY) return Promise.resolve(false);
  return runWorkspaceTrustPrompt(cwd, new ProcessTerminal(), process.stdin);
}

/** Terminal injection keeps the startup lifecycle testable without replacing process I/O. */
export function runWorkspaceTrustPrompt(cwd: string, terminal: Terminal, input: Readable): Promise<boolean> {
  if (input.destroyed || input.readableEnded) return Promise.resolve(false);
  return new Promise((resolve, reject) => {
    const tui = new TUI(terminal);
    let selected = 0;
    let detailsOffset = 0;
    let finished = false;
    // JSON escaping keeps directory names from injecting terminal controls.
    const path = JSON.stringify(cwd).replace(/[\u007f-\u009f]/g, char => `\\u${char.charCodeAt(0).toString(16).padStart(4, "0")}`);
    const title = "┌─ Workspace trust";
    const details = [
      "Only continue if you trust this folder's contents.",
      "Devo may read and edit files, execute commands or MCP servers,",
      "and contact model services with your account's access.",
      "Project instructions and configuration can influence these actions.",
      "Trust does not enable a sandbox or change your permission settings.",
    ];
    const layout = (width: number) => {
      const wrap = (lines: string[]) => lines.flatMap(line => wrapTextWithAnsi(line, Math.max(1, width - 1)));
      const choices = [`${selected === 0 ? "›" : " "} No, exit`, `${selected === 1 ? "›" : " "} Yes, I trust this folder`];
      const controls = "↑/↓ to choose · Enter to confirm · Esc to cancel";
      const full = wrap(["", title, path, "", ...details, "", "Trust this workspace?", "", ...choices, "", controls]);
      const rows = terminal.rows;
      if (full.length <= rows) return { lines: full, pageRows: 0, maxOffset: 0, canAccept: true };
      const dense = wrap([title, path, ...details, ...choices, controls]);
      if (dense.length <= rows) return { lines: dense, pageRows: 0, maxOffset: 0, canAccept: true };

      const header = wrap([title]);
      const decisions = wrap(["Trust lets Devo use tools and model services.", "Trust does not enable a sandbox.", ...choices,
        "PgUp/PgDn details · ↑/↓ choose · Enter/Esc"]);
      const pageRows = rows - header.length - decisions.length;
      if (pageRows < 1) {
        return { lines: wrap(["Resize to review workspace trust. Esc cancels."]).slice(0, rows), pageRows: 0, maxOffset: 0, canAccept: false };
      }
      const context = wrap([path, "", ...details]);
      const maxOffset = Math.max(0, context.length - pageRows);
      const offset = Math.min(detailsOffset, maxOffset);
      return { lines: [...header, ...context.slice(offset, offset + pageRows), ...decisions], pageRows, maxOffset, canAccept: true };
    };
    const finish = (accepted: boolean) => {
      if (finished) return;
      finished = true;
      input.off("end", cancel);
      input.off("close", cancel);
      input.off("error", cancel);
      try {
        tui.stop();
        resolve(accepted);
      } catch (error) {
        reject(error);
      }
    };
    const cancel = () => finish(false);
    const component: Component = {
      invalidate() {},
      render(width) {
        return layout(width).lines;
      },
      handleInput(data) {
        if (finished) return;
        // Pasted text (including embedded arrows/Enter) is not a trust decision.
        if (data.startsWith("\x1b[200~")) return;
        if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c") || matchesKey(data, "ctrl+d")) {
          cancel();
        } else if (matchesKey(data, "pageUp") || matchesKey(data, "pageDown")) {
          const { pageRows, maxOffset } = layout(terminal.columns);
          const step = Math.max(1, pageRows - 1);
          const offset = Math.min(detailsOffset, maxOffset);
          detailsOffset = Math.max(0, Math.min(maxOffset, offset + (matchesKey(data, "pageUp") ? -step : step)));
        } else if (matchesKey(data, "up") || matchesKey(data, "down")) {
          if (layout(terminal.columns).canAccept) selected = 1 - selected;
        } else if (matchesKey(data, "enter")) {
          if (layout(terminal.columns).canAccept) finish(selected === 1);
        }
      },
    };
    tui.addChild(component);
    tui.setFocus(component);
    input.once("end", cancel);
    input.once("close", cancel);
    input.once("error", cancel);
    try {
      tui.start();
    } catch (error) {
      finish(false);
    }
  });
}
