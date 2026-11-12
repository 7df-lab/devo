# Provider protocol audit

Audit baseline: upstream `main` at `1d2d46fa377c936edd8ad2f6213d87164dd69ac3`.
Documentation checked on 2026-11-12. This is a source-and-fixture audit of Devo's
text/tool adapters, not a claim that every optional vendor capability is supported
or that live paid API calls were exercised.

## API boundaries

- OpenAI: `/v1/chat/completions` and `/v1/responses`; public API references are
  rolling documentation, not a date-versioned request header. Responses also
  supports existing Codex-compatible endpoints. Compatible vendor extensions are
  not assumed to be part of the OpenAI contract.
- Anthropic: `/v1/messages`, retaining Devo's `anthropic-version: 2023-06-01`
  default. New documentation and model features do not justify silently changing
  an application's pinned API version. Explicit request/header overrides remain
  the compatibility escape hatch.

## Corrections

### Responses

- `response.incomplete` is a terminal event, not an unknown notification. Decode
  its output and usage and distinguish `max_output_tokens` from `content_filter`
  through `incomplete_details.reason`.
- Transport EOF without a terminal response must not manufacture a successful
  assistant turn. The explicit `[DONE]` marker remains accepted for compatible
  backends; official Responses uses typed terminal events.
- Preserve reasoning output items and replay them as top-level input items when
  `store: false`. Prefer the complete item from `response.output_item.done`, never
  an in-progress encrypted fragment. Current documentation says encrypted content
  is returned by default in stateless mode; omission of the legacy `include`
  setting is not itself a defect.
- Recover authoritative completed output items when a compatible endpoint omits
  output from the terminal envelope, including completed function arguments.
- Preserve refusal content instead of silently presenting an empty answer.
- Reject nonempty malformed or non-object completed function arguments instead
  of replacing them with `{}` and allowing a different tool invocation.
- Treat a failed status inside a compatible `response.done` envelope as failure.

### Anthropic Messages

- Keep server-tool `pause_turn` distinct from a completed turn. The agent loop
  resubmits the assistant content without injecting a synthetic user message.
- Serialize web-fetch citations as an object with `enabled`, not a bare boolean.
- Supply a valid manual thinking budget below `max_tokens` on older supported
  Claude models, and adaptive thinking on models that require it. Filter
  incompatible effort/sampling combinations without changing compatible vendors.
- Preserve signed and redacted thinking for tool-turn replay.
- Surface typed SSE errors and reject unfinished streams and invalid complete
  tool-input JSON. Preserve initial block text and optional thinking-token usage.
- Do not replay unsupported `status` fields in hosted search/fetch result blocks.
- The 2026-10-21 API removes sampling controls; the retained 2023-06-01 version
  still supports them for compatible model/thinking combinations. Optional new
  response usage details are tolerated, never required.

### Chat Completions

- Request and response corrections cover reasoning-model token limits and
  sampling, nullable and incremental log probabilities, selected-choice
  isolation, refusal visibility, and complete tool-argument validation.
- Responses and Chat function schemas are separate contracts: current Responses
  supports `output_schema`; that does not make it a valid Chat function field.

## Intentional limits and follow-up boundaries

- The existing Responses adapter omits `max_output_tokens` for Codex-compatible
  backends which reject it. This remains a limitation for ordinary public
  Responses requests: `ModelRequest.max_tokens` is not automatically enforced on
  that transport. An explicit `extra_body.max_output_tokens` can be used for a
  backend that supports it. A future explicit backend capability should separate
  these cases, rather than infer behavior from model names.
- OpenAI has no standalone public `web_fetch` tool. The generic adapter's
  `web_fetch`/`web_fetch_options` extension is not a supported OpenAI feature;
  public OpenAI browsing uses web search actions such as opening a page. No
  automatic substitution between differently configured tools is performed here.
- Generic provider-specific request fields are still passed through `extra_body`.
  Explicit overrides may violate a particular endpoint's schema and remain the
  caller's responsibility.
- New optional APIs such as multimodal generation, custom tools, asynchronous
  tools and new model-specific controls are not automatically implemented by
  this audit. A new API-version rollout requires its own compatibility testing.
- `[DONE]` compatibility does not imply acceptance of arbitrary truncated JSON.
  Stream errors and invalid complete local-tool arguments remain failures.

## Official references

- [OpenAI Chat create](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)
- [OpenAI Responses create](https://developers.openai.com/api/reference/resources/responses/methods/create)
- [Responses streaming events](https://developers.openai.com/api/reference/resources/responses/streaming-events)
- [Reasoning, including stateless replay](https://developers.openai.com/api/docs/guides/reasoning)
- [Function calling and schema differences](https://developers.openai.com/api/docs/guides/function-calling)
- [Claude Messages](https://platform.claude.com/docs/en/api/messages)
- [Claude streaming](https://platform.claude.com/docs/en/build-with-claude/streaming)
- [Claude stop reasons](https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons)
- [Claude extended thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking)
- [Claude version migration](https://platform.claude.com/docs/en/api/migration-2026-10-21)
- [Claude web fetch citations](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-fetch-tool)

## Verification

Regression tests use local HTTP/SSE fixtures and unit tests. No production
credentials are needed. Relevant checks are `cargo test -p devo-provider`,
`cargo test -p devo-core paused_server_turn`, `cargo fmt --all -- --check`, and
workspace CI (check, test, Clippy, generated-protocol consistency and platforms).
Local Rust execution was unavailable in the editing environment; actual CI
results, rather than this checklist, determine readiness to merge.
