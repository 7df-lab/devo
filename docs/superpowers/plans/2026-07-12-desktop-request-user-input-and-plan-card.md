# Desktop Request User Input and Plan Card Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make real `request_user_input` server events interactive in Desktop and make the composer todo card opaque with state-correct chevrons.

**Architecture:** Normalize the server's tagged `ServerEvent` wire shape at the Desktop SDK boundary and continue emitting the existing `question.asked` UI event, preserving the renderer's question atoms and response API. Keep the visual changes local to the existing `ChatQuestionFlow` and `SessionTaskList` components so protocol behavior and presentation remain independently testable.

**Tech Stack:** TypeScript, React 19, Bun test, Tailwind CSS, Lucide icons, Jotai.

## Global Constraints

- Preserve all pre-existing uncommitted work and do not stage or commit without an explicit request.
- Keep renderer inline icons at `size-3.5` with `stroke-[1.5]` unless an established local pattern requires otherwise.
- Do not modify generated protocol types by hand.

---

### Task 1: Normalize real request-user-input events

**Files:**
- Modify: `apps/desktop/packages/devo-ai-sdk/src/v2/acp-client-support.ts`
- Modify: `apps/desktop/packages/devo-ai-sdk/src/v2/client.ts`
- Test: `apps/desktop/packages/devo-ai-sdk/src/v2/client.test.ts`

**Interfaces:**
- Consumes: `_meta["devo/originalEvent"]` values serialized as `{ kind: "request_user_input", request, questions }` and legacy `{ RequestUserInput: payload }` values.
- Produces: the existing `question.asked` event and `_devo/request_user_input/respond` request path.

- [x] **Step 1: Change the SDK test fixture to the actual tagged wire shape**

```ts
"devo/originalEvent": {
  kind: "request_user_input",
  request: {
    request_id: "rq1",
    session_id: "s1",
    turn_id: "t1",
    item_id: null,
  },
  questions: [
    {
      id: "scope",
      header: "Scope",
      question: "Which scope?",
      isOther: true,
      isSecret: false,
      options: [{ label: "Repo", description: "Current repository" }],
    },
  ],
}
```

- [x] **Step 2: Run the focused test and verify it fails before the fix**

Run: `bun test packages/devo-ai-sdk/src/v2/client.test.ts --test-name-pattern "maps original request_user_input"`

Expected: FAIL because no `question.asked` event is emitted for the tagged event.

Actual: the boundary regression test failed before implementation because `requestUserInputFromOriginalEvent` did not exist.

- [x] **Step 3: Add one boundary parser and use it from the client**

```ts
export function requestUserInputFromOriginalEvent(original: unknown): Record<string, unknown> | undefined {
  if (!original || typeof original !== "object") return undefined
  const event = original as Record<string, unknown>
  if (event.kind === "request_user_input") return event
  const legacy = event.RequestUserInput
  return legacy && typeof legacy === "object" ? legacy as Record<string, unknown> : undefined
}
```

Replace the legacy-only branch in `handleOriginalEvent` with a call to this parser and pass its result to `handleRequestUserInput`.

- [x] **Step 4: Run the focused SDK test and verify it passes**

Run: `bun test packages/devo-ai-sdk/src/v2/client.test.ts --test-name-pattern "maps original request_user_input"`

Expected: PASS, including the full `question.asked` payload and response request equality.

### Task 2: Polish the Desktop question card

**Files:**
- Modify: `apps/desktop/src/renderer/components/chat/chat-question.tsx`
- Verify: `apps/desktop/src/renderer/components/chat/chat-view.tsx`

**Interfaces:**
- Consumes: `QuestionRequest[]` from the existing Jotai question queue.
- Produces: a focused, keyboard-accessible, opaque card with option selection, custom input, progress, skip, back, and submit controls.

- [x] **Step 1: Refine the card hierarchy without changing response semantics**

Use an opaque `bg-card`, subtle shadow/ring treatment, a compact tinted icon container, a visible header label, and distinct option selected states. Keep the custom input and footer controls keyboard accessible.

- [x] **Step 2: Verify protocol fields map to the UI contract**

Confirm `isOther` controls custom answer visibility and `isSecret` selects a password input, while the current protocol remains single-select.

Actual: preset and custom answers are mutually exclusive, options expose radio semantics, and the custom input has an explicit accessible label.

- [x] **Step 3: Run Desktop type checking**

Run: `bun run check-types`

Expected: PASS with no TypeScript errors.

Actual: the command ran but remains blocked by four pre-existing errors in `message.tsx`, `process-timeline-view.tsx`, `session-view.tsx`, and `use-file-search.ts`; none point to this task's parser or card components.

### Task 3: Fix the composer todo card surface and arrow semantics

**Files:**
- Modify: `apps/desktop/src/renderer/components/chat/session-task-list.tsx`

**Interfaces:**
- Consumes: the existing `isExpanded` state and current session todos.
- Produces: an opaque todo card whose arrow points up when collapsed and down when expanded.

- [x] **Step 1: Make the visible card opaque**

Replace the translucent `bg-muted/10` surface with `bg-card` and retain a subtle border and hover state.

- [x] **Step 2: Correct the arrow direction and accessible state**

Use `ChevronDownIcon` when expanded and `ChevronUpIcon` when collapsed, add `aria-expanded={isExpanded}`, and keep the icon at `size-3.5 stroke-[1.5]`.

- [x] **Step 3: Run formatting and focused checks**

Run: `bunx biome check src/renderer/components/chat/chat-question.tsx src/renderer/components/chat/session-task-list.tsx packages/devo-ai-sdk/src/v2/acp-client-support.ts packages/devo-ai-sdk/src/v2/client.test.ts`

Expected: PASS.

Actual: focused source inspection and `git diff --check` pass. The repository's `bun run lint` command cannot start because the configured `biome` executable is not installed; an ad-hoc Biome binary does not resolve the repository's inherited monorepo configuration and reports existing whole-file diagnostics.

### Task 4: End-to-end verification

**Files:**
- Verify only: all files above.

**Interfaces:**
- Consumes: completed Tasks 1-3.
- Produces: evidence that the SDK event path, renderer types, and production Desktop build all remain healthy.

- [x] **Step 1: Run the SDK test file**

Run: `bun test packages/devo-ai-sdk/src/v2/client.test.ts`

Expected: all tests pass.

- [x] **Step 2: Run Desktop type checking and build**

Run: `bun run check-types && bun run build`

Expected: both commands pass.

Actual: the production Desktop build passes; type checking has the four pre-existing errors recorded in Task 2.

- [x] **Step 3: Check patch hygiene**

Run: `git diff --check`

Expected: no whitespace errors.
