# @devo/ui

## 0.6.0

### Patch Changes

- [`f7c84b5`](https://github.com/tsiao/devo/commit/f7c84b5b4162824de5b4d31e860a6658f875e65e) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Apply thin scrollbar styles globally to prevent ugly native scrollbars across all scrollable areas

## 0.5.3

### Patch Changes

- [`8171dee`](https://github.com/tsiao/devo/commit/8171deee90b638ea92604a20ab323da3f739b627) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix horizontal overflow and clipping in the desktop app at narrow window widths. Add `min-w-0` and `overflow-hidden` throughout the flex layout chain (SidebarInset, content area, conversation container, chat view, prompt toolbar) and make the session app bar collapse responsively with Tailwind breakpoints.

## 0.3.0

### Minor Changes

- [`594e4b7`](https://github.com/tsiao/devo/commit/594e4b7e299dee6ba507f990001f505f6afd22c5) Thanks [@ItsWendell](https://github.com/ItsWendell)! - ### New Features

  - **Session metrics**: comprehensive per-session work time, cost, tokens, model distribution, cache efficiency, and tool breakdown
  - **Per-turn metadata**: display model, duration, and cost after each response
  - **Live turn timer**: show elapsed time on submit button while agent is working
  - **Prompt toolbar selector**: enhanced model/provider selector UI
  - **Devo API integration**: replace disk-based discovery and messages with Devo server API

  ### Performance

  - **Per-session streaming**: scope streaming buffer and version notifications per session, eliminating cross-session re-renders (~20x/sec savings for inactive sessions)
  - **Decoupled volatile metrics from agent identity**: sidebar no longer re-renders on every metrics tick
  - **Lazy metrics for background sessions**: only the viewed session uses reactive atom subscriptions
  - **Granular sidebar subscriptions**: each project folder and session item subscribes independently

  ### UI/UX

  - **Base UI migration**: migrated shadcn components from Radix to Base UI
  - **Sidebar improvements**: stabilized sort order, relative last-active time, animated project folders
  - **New chat styling**: translucent backgrounds and subtle borders on suggestion cards
  - **Command dialog**: layout fix for better usability

  ### Fixes

  - Support Devo XML read output format in tool card parser
  - Migrate tooltips from Radix asChild to Base UI render prop pattern
  - Stabilize sidebar atom references to prevent cascading re-renders

## 0.2.0

### Patch Changes

- [`d2d6f2b`](https://github.com/tsiao/devo/commit/d2d6f2b3013ad0fa3bb9ac08ad9b8ff91517ffc5) Thanks [@ItsWendell](https://github.com/ItsWendell)! - ### New Features

  - Add provider management with icons, catalog, and onboarding integration
  - Add git worktree backend with lifecycle management and UI
  - Add automations subsystem with database, scheduler, and IPC (hidden for now)

  ### Improvements

  - Improve Electron main process reliability
  - Improve command palette animation and chat UX
  - Increase spacing between expanded tool call items in chat turns

  ### Fixes

  - Upgrade hono to 4.11.9 to resolve security alerts
  - Resolve type errors in chat-input component
