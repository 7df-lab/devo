# Desktop Reference Search and Session Refill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Desktop `@` search expose server-provided Skills and MCPs, and keep a paginated project session list full after a deletion.

**Architecture:** Preserve the server-owned `search/*` ranking and pass the complete typed result snapshot through the renderer hook into the mention popover. For deletion, update the connection manager's discovery cache at the same boundary that refills the current pagination window, then invoke that boundary after a confirmed delete.

**Tech Stack:** TypeScript, React, Jotai, Bun tests, Devo Desktop SDK

## Global Constraints

- Work in `/Users/tsiao/Desktop/devo` and preserve all unrelated dirty-worktree changes.
- Keep inline Desktop icons consistent with `apps/desktop/AGENTS.md` (`size-3.5 stroke-[1.5]`).
- Do not stage or commit changes unless the user asks.

---

### Task 1: Preserve and render all server reference-search result kinds

**Files:**
- Create: `apps/desktop/src/renderer/hooks/use-reference-search.ts`
- Delete: `apps/desktop/src/renderer/hooks/use-file-search.ts`
- Modify: `apps/desktop/src/renderer/components/chat/mention-popover.tsx`
- Modify: `apps/desktop/src/renderer/components/chat/prompt-mentions.ts`
- Modify: `apps/desktop/src/renderer/components/chat/context-items.tsx`
- Modify: `apps/desktop/src/renderer/components/new-chat.tsx`
- Modify: `apps/desktop/src/renderer/components/chat/chat-input.tsx`
- Modify: `apps/desktop/src/renderer/components/chat/chat-view.tsx`
- Test: `apps/desktop/src/renderer/components/chat/mention-popover.test.ts`

**Interfaces:**
- Consumes: `ReferenceSearchSnapshot.results: ReferenceSearchResult[]` from `@devo-ai/sdk/v2/client`.
- Produces: `useReferenceSearch(...): { results, isLoading, error }` and mention options for `skill`, `mcp`, `file`, and local `agent` entries.

- [x] **Step 1: Write a failing result-mapping test**

Assert that a snapshot containing Skill, MCP, and File results maps all three into Desktop mention options, retains each server `insert_text`, and preserves disabled metadata.

- [x] **Step 2: Run the focused test and verify the file-only implementation fails**

Run: `bun test src/renderer/components/chat/mention-popover.test.ts`

Expected: FAIL because the full-result mapping API and Skill/MCP options do not exist.

- [x] **Step 3: Replace the file-only hook with a full reference-result hook**

Subscribe to the existing connection-local search session, store `snapshot.results` without filtering by kind, and retain the existing debounce/cancel/error behavior.

- [x] **Step 4: Add typed Skill and MCP popover groups and selection behavior**

Map the server result fields directly, render category-appropriate icons/labels/descriptions, prevent disabled results from being selected, and insert the exact server `insert_text` token while retaining mention tracking.

- [x] **Step 5: Run the focused renderer test**

Run: `bun test src/renderer/components/chat/mention-popover.test.ts`

Expected: PASS.

### Task 2: Refill the current session page after deletion

**Files:**
- Modify: `apps/desktop/src/renderer/services/connection-manager.ts`
- Modify: `apps/desktop/src/renderer/components/sidebar-layout.tsx`
- Test: `apps/desktop/src/renderer/services/connection-manager.test.ts`

**Interfaces:**
- Consumes: `projectPaginationFamily(directory).currentLimit` and the discovery-session cache.
- Produces: `refillProjectSessionsAfterDelete(projectDirectory: string, sessionId: string): Promise<void>`.

- [x] **Step 1: Write a failing pagination regression test**

Seed six root sessions, load a five-session window, delete one visible session, and assert that the sixth session is hydrated while `currentLimit` remains five.

- [x] **Step 2: Run the focused test and verify it fails**

Run: `bun test src/renderer/services/connection-manager.test.ts`

Expected: FAIL because deletion neither updates the discovery cache nor refills the current window.

- [x] **Step 3: Implement the cache-aware refill boundary**

Remove the deleted ID from renderer state and `discoveredSessions`, read the existing project limit, and reload the same root-session window so the next hidden session fills the gap without making “Show more” jump an extra page.

- [x] **Step 4: Invoke refill after a confirmed sidebar deletion**

Call the connection-manager boundary with `deleteTarget.projectDirectory` only after the server delete succeeds, before completing navigation/dialog cleanup.

- [x] **Step 5: Run the focused pagination test**

Run: `bun test src/renderer/services/connection-manager.test.ts`

Expected: PASS.

### Task 3: Verify the combined Desktop changes

**Files:**
- Verify only; no additional production files expected.

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces: focused regression evidence and repository hygiene evidence.

- [x] **Step 1: Run both focused suites together**

Run: `bun test src/renderer/components/chat/mention-popover.test.ts src/renderer/services/connection-manager.test.ts`

Expected: all tests pass.

- [x] **Step 2: Run Desktop type checking and production build**

Run: `bun run check-types`

Run: `bun run build`

Expected: no new errors in touched files; record any unrelated dirty-tree failures exactly.

- [x] **Step 3: Run diff hygiene**

Run: `git diff --check`

Expected: exit 0.

## Verification Record

- Focused suites: 7 tests passed across `mention-popover.test.ts` and `connection-manager.test.ts`.
- Production build: passed for main, preload, and renderer bundles.
- Type checking: no errors in task files; the dirty worktree still has three unrelated errors in `packages/ui/src/components/ai-elements/message.tsx`, `src/renderer/components/chat/process-timeline-view.tsx`, and `src/renderer/components/session-view.tsx`.
- Lint: unavailable because the configured `biome` executable is not installed (`/bin/bash: biome: command not found`).
- `git diff --check`: passed.
