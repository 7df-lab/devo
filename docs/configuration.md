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

- `model_slug` selects Devo's local model metadata from `models.json`.
- `provider` selects the configured connection record.
- `request_model` is the provider-specific model string sent on the wire.
- `invocation_method` selects the provider protocol, such as
  [`openai_chat_completions`](https://developers.openai.com/api/reference/chat-completions/overview),
  [`openai_responses`](https://developers.openai.com/api/reference/responses/overview),
  or [`anthropic_messages`](https://platform.claude.com/docs/en/api/messages).

Existing configuration using `model_name` remains readable. Devo writes the
field as `request_model` the next time that binding is saved.

## Custom Models

If the model you want to use is not in the built-in list, add it to
`models.json`, then bind it through `config.toml`.

User-level model catalog:

- macOS/Linux: `~/.devo/models.json`
- Windows: `C:\Users\yourname\.devo\models.json`

Project-level overrides can also be placed at `<workspace>/.devo/models.json`.
Catalog precedence is `<workspace>/.devo/models.json`, then
`<DEVO_HOME>/models.json`, then the built-in catalog.
In `models.json`, `provider` is the default wire API metadata for the model; the
actual endpoint is still selected by the `provider` field in `config.toml`.
If `base_instructions` is omitted, Devo falls back to the built-in default base
instructions. An explicit empty string (`""`) means the model has no base
instructions.

Example `models.json` entry:

```json
[
  {
    "slug": "my-coding-model",
    "display_name": "My Coding Model",
    "channel": "Custom",
    "provider": "openai_chat_completions",
    "description": "Custom OpenAI-compatible coding model.",
    "reasoning_capability": "unsupported",
    "context_window": 200000,
    "effective_context_window_percent": 95,
    "max_tokens": 4096,
    "input_modalities": ["text"],
    "base_instructions": "You are Devo, a coding agent. Help the user edit and understand code."
  }
]
```

Then reference that `slug` from a model binding:

```toml
[model_bindings.my-coding-model-example]
enabled = true
model_slug = "my-coding-model"
provider = "my.provider"
request_model = "provider-specific-model-name"
display_name = "My Coding Model"
invocation_method = "openai_chat_completions"
```
