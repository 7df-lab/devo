# Configuration

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` is the recommended setup path. For manual configuration, Devo
merges settings in this order:

1. Built-in defaults
2. `DEVO_HOME/config.toml` - user-level config, defaulting to `~/.devo/config.toml`
   on macOS/Linux and `C:\Users\yourname\.devo\config.toml` on Windows
3. `<workspace>/.devo/config.toml` - project-level config
4. CLI flags

Credentials live separately in `DEVO_HOME/auth.json`; `config.toml` should refer
to credential ids instead of storing API keys directly.

Minimal shape:

```toml
[defaults]
model_binding = "deepseek-v4-flash-api-deepseek-com"

[providers."api.deepseek.com"]
enabled = true
name = "api.deepseek.com"
base_url = "https://api.deepseek.com"
credential = "api_deepseek_com_api_key"
wire_apis = ["openai_chat_completions"]

[model_bindings.deepseek-v4-flash-api-deepseek-com]
enabled = true
model_slug = "deepseek-v4-flash"
provider = "api.deepseek.com"
request_model = "deepseek-v4-flash"
display_name = "DeepSeek V4 Flash"
invocation_method = "openai_chat_completions"
default_reasoning_effort = "high"
```

The important separation is:

- `model_slug` selects Devo's local model metadata by slug.
- The binding's `provider` selects a `[providers.<id>]` connection record.
- `request_model` is the provider-facing model id sent on the wire.
- `invocation_method` selects the operational provider protocol, such as
  [`openai_chat_completions`](https://developers.openai.com/api/reference/chat-completions/overview),
  [`openai_responses`](https://developers.openai.com/api/reference/responses/overview),
  or [`anthropic_messages`](https://platform.claude.com/docs/en/api/messages).

Model metadata also has a `provider` field. It describes the wire API the model
expects, while the binding's `invocation_method` chooses the connection used at
runtime; keep those values aligned. API keys remain in `auth.json` and are
connected through the provider's `credential` reference.

Existing configuration using `model_name` remains readable. Devo writes the
field as `request_model` the next time that binding is saved.

## Model Metadata and Custom Models

Configure model metadata in user or workspace `config.toml` under
`[model.<slug>]`. A section for a built-in slug is a partial override: omitted
fields retain their built-in values. A new slug creates a custom model with safe
defaults, which should then be connected through both `[providers.<id>]` and
`[model_bindings.<id>]`.

For example, this changes only the built-in context window:

```toml
[model.qwen3-coder-next]
context_window = 262144
effective_context_window_percent = 90
```

The exact effective context formula is
`context_window * effective_context_window_percent / 100`; the result is the
context available to the model and the automatic-compaction boundary. For a
custom model and connection:

```toml
[defaults]
model_binding = "my-coding-model-example"

[model.my-coding-model]
display_name = "My Coding Model"
description = "Custom OpenAI-compatible coding model."
channel = "Custom"
provider = "openai_chat_completions"
context_window = 200000
effective_context_window_percent = 95
max_tokens = 4096
temperature = 0.2
top_p = 0.9
top_k = 40.0
reasoning_capability = { levels = ["low", "medium", "high"] }
reasoning_implementation = "request_parameter"
default_reasoning_effort = "medium"
base_instructions = "You are Devo, a coding agent. Help the user edit and understand code."
input_modalities = ["text", "image"]
truncation_policy = { mode = "tokens", limit = 12000 }
supports_image_detail_original = true

[providers.my-provider]
enabled = true
name = "My Provider"
base_url = "https://api.example.com/v1"
credential = "my_provider_api_key"
wire_apis = ["openai_chat_completions"]

[model_bindings.my-coding-model-example]
enabled = true
model_slug = "my-coding-model"
provider = "my-provider"
request_model = "provider-specific-model-name"
display_name = "My Coding Model"
invocation_method = "openai_chat_completions"
```

Configurable metadata includes `display_name`, the picker-facing model name;
`description`, explanatory text shown to users; and `channel`, the grouping
label used to organize models. `context_window` and
`effective_context_window_percent` determine effective context, while
`max_tokens` is the default response-output limit. Sampling defaults are
`temperature` for randomness, `top_p` for nucleus probability mass, and `top_k`
for the candidate-token cap. The `provider` wire API is one of
`openai_chat_completions`, `openai_responses`, or `anthropic_messages`.
Reasoning metadata is typed: `reasoning_capability` can be `unsupported`,
`toggle`, `{ levels = [...] }`, or `{ togglewithlevels = [...] }`;
`reasoning_implementation` can be `disabled`, `request_parameter`, or a typed
`model_variant` table. A model variant maps a logical reasoning selection to a
different provider-facing model id, optional effective effort, and optional
extra request body instead of changing a parameter on the same model;
`default_reasoning_effort` selects the default typed effort. `input_modalities`
accepts `text` and `image`; `truncation_policy` chooses a byte or token limit for
oversized tool-result content before it is included in a model request; and
`supports_image_detail_original` enables original image detail.

Omitting `base_instructions` retains built-in instructions for a built-in model
or uses Devo's default instructions for a custom model. An explicit empty string
(`base_instructions = ""`) means no base instructions.

Legacy `model = "slug"` remains readable. Because `[model.<slug>]` now owns the
top-level `model` table namespace, new configuration must select the active
connection with `[defaults].model_binding` instead of the legacy scalar key.

### TUI Preferences

Top-level keys in `DEVO_HOME/config.toml` also store a few UI preferences:

```toml
theme = "aurora"
collapse_reasoning = true
```

- `theme` selects the TUI color theme (also set via `/theme`).
- `collapse_reasoning` controls reasoning display (also set via `/show-reasoning`):
  - `true` (default): while streaming, show only the latest 3 lines; when finished, keep short
    reasoning in full and collapse longer reasoning to a one-line `Thought · …`
    summary (full text remains available in Ctrl+T).
  - `false`: show full reasoning while streaming and after it finishes.

### Migrating from `models.json`

Old `~/.devo/models.json` and `<workspace>/.devo/models.json` files are ignored.
Manually copy the fields you still want into `[model.<slug>]` sections in the
user or workspace `config.toml`, then add or retain the matching provider and
model binding. Keep API keys in `auth.json`; refer to them from
`[providers.<id>].credential`.
