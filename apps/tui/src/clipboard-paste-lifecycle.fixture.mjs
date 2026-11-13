// Runs in its own process so platform clipboard mocks cannot leak to other tests.
import { mock } from "node:test";
import assert from "node:assert/strict";

const base = new URL("../lib/coding-agent/src/", import.meta.url);
let release;
let pending;
let stage;
const image = { bytes: Buffer.from("placeholder"), mimeType: "image/png" };
mock.module(new URL("utils/clipboard-image.ts", base).href, {
  namedExports: {
    readClipboardImage: () => stage === "read" ? pending : Promise.resolve(image),
    isWaylandSession: () => false,
  },
});
mock.module(new URL("utils/image-resize.ts", base).href, {
  namedExports: { resizeImage: () => stage === "resize" ? pending : Promise.resolve(null) },
});
const { InteractiveMode } = await import(new URL("modes/interactive/interactive-mode.ts", base));
const { initTheme } = await import(new URL("modes/interactive/theme/theme.ts", base));
initTheme("dark");

for (stage of ["read", "resize"]) {
  for (const scenario of ["normal", "typing", "send", "switch", "fork", "editor-replaced",
    "stopped", "stash", "restore", "tree-restore", "tree-preserve", "clear", "queue"]) {
    pending = new Promise(resolve => { release = resolve; });
    const mode = Object.create(InteractiveMode.prototype);
    let text = ["restore", "tree-restore"].includes(scenario) ? "" : "before";
    const editor = {
      getText: () => text,
      setText: value => { text = value; },
      insertTextAtCursor: value => { text += value; },
      clearHistory() {},
    };
    const noop = () => {};
    const prompts = [];
    Object.assign(mode, {
      editor, defaultEditor: editor, nextImageMarkerId: 1, pastedImages: new Map(),
      rememberPastedImage(id, img) { this.pastedImages.set(id, img); },
      ui: { requestRender: noop }, getCurrentModel: () => ({ input: ["image"] }), showStatus: noop,
      promptStashState: {}, promptStashSessionId: "session-a", submittedInputBehavior: "steer",
      inputSubmissionsPending: 0, pendingPromptStashReleases: [], clearShortcutGuide: noop,
      clearSideQuestion: noop, flushPendingBashComponents: noop, clearEscapeRepeat: noop,
      clearCtrlCExitHint: noop, treeNavigationRenderGeneration: 0,
      sessionEventQueue: Promise.resolve(), sessionEventGeneration: 0,
      inputSubmissionGeneration: 0, clipboardPasteGeneration: 0, activityStopped: false,
      chatContainer: { clear: noop }, shortcutGuideContainer: { clear: noop },
      pendingMessagesContainer: { clear: noop }, queuedMessagesContainer: { clear: noop },
      queueSelection: { reset: noop }, liveImageMarkerIds: () => new Set(),
      discardRefineLoader: noop, activityTracker: { reset: noop }, resetPendingToolState: noop,
      agentRunFileChanges: new Map(), renderRecap: noop, ipythonToolComponents: new Map(),
      lateIpythonSentAgentMessages: new Map(), resetSubagentSummary: noop,
      setGoalAnnouncementBaseline: noop, syncGoalTray: noop, getGoalState: () => null,
      renderInitialMessages: async () => {}, updatePendingMessagesDisplay: noop,
      syncWorkingLoader: noop, stopWorkingLoader: noop, withActivityTransition: fn => fn(),
      agentConnection: {
        prompt: async value => { prompts.push(value); },
        switchSession: async () => ({ cancelled: false }),
        getSessionTree: async () => ({ leafId: "leaf" }),
        fork: async () => ({ cancelled: false }),
      },
    });
    const paste = mode.handleClipboardImagePaste();
    // Let the read resolve and conversion start for the second-stage cases.
    await new Promise(resolve => setImmediate(resolve));
    if (scenario === "stash") mode.handlePromptStash();
    if (scenario === "restore") {
      mode.promptStash = { text: "restored" };
      mode.restorePromptStashIfEditorEmpty();
    }
    if (scenario.startsWith("tree-")) await mode.renderTreeNavigation({ editorText: "selected" });
    if (scenario === "typing") editor.setText("typed while waiting");
    // These two cases model lifecycle signals; the remaining transitions call production methods.
    if (scenario === "editor-replaced") mode.editor = { ...editor };
    if (scenario === "stopped") mode.activityStopped = true;
    if (scenario === "send") {
      mode.setupEditorSubmitHandler();
      await editor.onSubmit("before");
      assert.deepEqual(prompts, ["before"]);
    }
    if (scenario === "switch") await mode.handleResumeSession("new");
    if (scenario === "fork") await mode.handleCloneCommand();
    if (scenario === "clear") mode.clearInputBar();
    if (scenario === "queue") mode.setEditorTextFromQueueSelection("queued draft");
    const before = text;
    release(stage === "read" ? image : null);
    await paste;
    const keep = ["normal", "typing", "tree-preserve"].includes(scenario);
    assert.deepEqual({ text, images: mode.pastedImages.size }, {
      text: keep ? before + "[image #1]" : before, images: keep ? 1 : 0,
    }, `${stage}/${scenario}`);
    console.log(`${stage}/${scenario}: verified`);
  }
}
