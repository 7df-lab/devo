# Fast Initialize Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make ACP `initialize` complete quickly by preventing repeated synchronous construction of equivalent provider HTTP clients during server bootstrap.

**Architecture:** Add a process-wide cache in `devo-provider` that owns one request client and one streaming client per proxy configuration. Provider adapters continue to own their headers, credentials, and endpoint configuration, while cloning the cached `reqwest::Client` handles so they share connection pools and avoid repeated native TLS initialization.

**Tech Stack:** Rust, reqwest, anyhow, Cargo tests.

## Global Constraints

- Preserve existing provider and proxy behavior, including custom per-provider headers.
- Do not change ACP timeout values; remove the work that causes the timeout.
- Do not modify existing user changes outside the provider HTTP module and this plan.

---

### Task 1: Cache equivalent provider HTTP clients

**Files:**
- Modify: `crates/provider/src/http.rs`
- Test: `crates/provider/src/http.rs`

**Interfaces:**
- Consumes: `ProviderHttpOptions::network_proxy`, `NetworkProxyConfig`, and the existing request/streaming client builders.
- Produces: unchanged `ProviderHttpOptions::build_request_client() -> Result<Client>` and `build_streaming_client() -> Result<Client>` behavior backed by shared cached clients.

- [x] **Step 1: Write a failing unit test**

  Add a cache-level test that requests the same client kind twice for one proxy configuration, increments an `AtomicUsize` in the builder closure, and asserts the builder ran once.

- [x] **Step 2: Run the test to verify it fails**

  Run `cargo test -p devo-provider http::tests::http_client_cache_reuses_equivalent_clients -- --exact` and confirm the missing cache implementation fails to compile.

- [x] **Step 3: Implement the minimal cache**

  Add an `HttpClientCache` guarded by `OnceLock<Mutex<_>>`. Store request and streaming clients separately as `(NetworkProxyConfig, Client)` entries, return cheap `Client` clones on a hit, and build exactly once on a miss.

- [x] **Step 4: Run focused provider tests**

  Run the new unit test and `cargo test -p devo-provider --test provider_http`; both must pass, preserving headers and explicit proxy routing.

### Task 2: Verify startup behavior

**Files:**
- No additional source changes expected.

**Interfaces:**
- Consumes: the current multi-provider `~/.devo/config.toml` through an isolated temporary `DEVO_HOME`.
- Produces: measured config-load-to-database-open latency below the 10-second ACP response timeout, with `initialize` accepted successfully.

- [x] **Step 1: Build the current binary**

  Run `cargo build -p devo-cli` and allow Rust compilation to finish normally.

- [x] **Step 2: Benchmark isolated startup**

  Copy the current config, auth, and model catalog into a temporary home, send one ACP `initialize` request to `target/debug/devo server --transport stdio`, and compare timestamps between `loaded server config`, `opening database`, and `accepted ACP initialize request`.

  Measured result: config-load-to-database-open improved from approximately 12.8 seconds to 435 milliseconds, and ACP `initialize` was accepted at approximately 449 milliseconds.

- [x] **Step 3: Run final checks**

  Run `cargo test -p devo-provider`, `cargo test -p devo-server provider_config`, and `git diff --check`. Confirm no unrelated dirty-tree files changed.
