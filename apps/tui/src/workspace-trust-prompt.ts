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
    let finished = false;
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
        // JSON escaping keeps directory names from injecting terminal controls.
        const path = JSON.stringify(cwd).replace(/[\u007f-\u009f]/g, char => `\\u${char.charCodeAt(0).toString(16).padStart(4, "0")}`);
        return [
          "", "┌─ Workspace trust", path, "",
          "Only continue if you trust this folder's contents.",
          "Devo may read and edit files, execute commands or MCP servers,",
          "and contact model services with your account's access.",
          "Project instructions and configuration can influence these actions.",
          "Trust does not enable a sandbox or change your permission settings.",
          "", "Trust this workspace?", "",
          `${selected === 0 ? "›" : " "} No, exit`,
          `${selected === 1 ? "›" : " "} Yes, I trust this folder`,
          "", "↑/↓ to choose · Enter to confirm · Esc to cancel",
        ].flatMap(line => wrapTextWithAnsi(line, Math.max(1, width - 1)));
      },
      handleInput(data) {
        if (finished) return;
        // Pasted text (including embedded arrows/Enter) is not a trust decision.
        if (data.startsWith("\x1b[200~")) return;
        if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c") || matchesKey(data, "ctrl+d")) {
          cancel();
        } else if (matchesKey(data, "up") || matchesKey(data, "down")) {
          selected = 1 - selected;
        } else if (matchesKey(data, "enter")) {
          finish(selected === 1);
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
