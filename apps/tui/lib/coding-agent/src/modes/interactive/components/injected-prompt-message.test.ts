import assert from "node:assert/strict";
import { test } from "node:test";
import { IPYTHON_STATE_RESTORED_CUSTOM_TYPE } from "../../../core/messages.js";
import { initTheme } from "../theme/theme.js";
import { InjectedPromptMessageComponent } from "./injected-prompt-message.js";

initTheme("dark");

test("Python kernel state notices render restored and fresh labels", () => {
  const restored = new InjectedPromptMessageComponent({
    role: "custom",
    customType: IPYTHON_STATE_RESTORED_CUSTOM_TYPE,
    content: "",
    display: true,
    details: { restored: true },
    timestamp: 1,
  });
  const fresh = new InjectedPromptMessageComponent({
    role: "custom",
    customType: IPYTHON_STATE_RESTORED_CUSTOM_TYPE,
    content: "",
    display: true,
    details: { restored: false },
    timestamp: 2,
  });

  assert.match(restored.render(80).join("\n"), /Restored Python kernel state/);
  assert.match(fresh.render(80).join("\n"), /Started fresh Python kernel/);
});
