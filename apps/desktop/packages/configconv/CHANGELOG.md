# @devo/configconv

## 0.5.2

### Patch Changes

- [`098847c`](https://github.com/tsiao/devo/commit/098847c404f51c0954ffaba1c872910b93dd69d9) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix prototype-polluting assignment vulnerability in Cursor permission converter

## 0.5.0

### Minor Changes

- [`755242d`](https://github.com/tsiao/devo/commit/755242d87f361457d00f3d56b91002f5ee1a7a6e) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Migrate history writer to SQLite for Devo v1.2.0+. Sessions, messages, and parts are now written directly to the SQLite database at `~/.local/share/devo/devo.db`. Falls back to legacy flat-file JSON storage when no database exists.

## 0.4.0

### Patch Changes

- [`2a9d3de`](https://github.com/tsiao/devo/commit/2a9d3de3b529a6aa73b8ae574fb2a6f2084d73f9) Thanks [@ItsWendell](https://github.com/ItsWendell)! - ### Breaking: Require Devo >= 1.2.0

  Devo now requires Devo 1.2.0 or higher. Older versions will be blocked during the environment check.

  ### New Features

  - **Incremental streaming (message.part.delta)**: handle the new `message.part.delta` SSE event for incremental text/reasoning updates, replacing full part object replacements with efficient string delta appending
  - **Remote server support**: connect to remote Devo servers with authentication, mDNS discovery, and onboarding integration
  - **Update banner redesign**: floating toast card instead of full-width bar

  ### SDK Migration

  - Upgrade `@devo-ai/sdk` from 1.1.x to 1.2.x across all packages
  - Migrate all SDK type imports from v1 to v2, gaining proper typed discriminated unions for all events
  - `Permission` type replaced by `PermissionRequest` (field `title` becomes `permission`)
  - `permission.updated` event replaced by `permission.asked`
  - Remove unnecessary type casts throughout the event processing pipeline

  ### Fixes

  - Fix CI release workflow to run builds in a single workflow run
  - Suppress hover background on macOS sidebar server indicator
