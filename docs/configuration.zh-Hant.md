# 配置

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` 是推薦的設定路徑。如需手動配置，Devo 會按以下順序合併設定：

1. 內建預設值
2. `DEVO_HOME/config.toml` - 使用者級配置，預設在 macOS/Linux 上為
   `~/.devo/config.toml`，在 Windows 上為 `C:\Users\yourname\.devo\config.toml`
3. `<workspace>/.devo/config.toml` - 專案級配置
4. CLI flags

憑據單獨保存在 `DEVO_HOME/auth.json`；`config.toml` 應引用 credential id，
而不是直接儲存 API key。

最小結構：

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

關鍵區分如下：

- `model_slug` 從 `models.json` 中選擇 Devo 的本地模型中繼資料。
- `provider` 選擇已配置的連線記錄。
- `request_model` 是傳送到 provider 的特定模型字串。
- `invocation_method` 選擇 provider 協議，例如
  [`openai_chat_completions`](https://developers.openai.com/api/reference/chat-completions/overview)、
  [`openai_responses`](https://developers.openai.com/api/reference/responses/overview)，
  或 [`anthropic_messages`](https://platform.claude.com/docs/en/api/messages)。

## 自訂模型

如果想使用的模型不在內建清單中，請將它加入 `models.json`，然後透過
`config.toml` 綁定。

使用者級模型目錄：

- macOS/Linux: `~/.devo/models.json`
- Windows: `C:\Users\yourname\.devo\models.json`

專案級覆蓋也可以放在 `<workspace>/.devo/models.json`。
在 `models.json` 中，`provider` 是該模型的預設 wire API 中繼資料；實際端點仍由
`config.toml` 中的 `provider` 欄位選擇。
若省略 `base_instructions`，Devo 會回退到內建預設 base instructions；明確寫空字串
（`""`）表示該模型不使用 base instructions。

範例 `models.json` 條目：

```json
[
  {
    "slug": "my-coding-model",
    "display_name": "My Coding Model",
    "channel": "Custom",
    "provider": "openai_chat_completions",
    "description": "Custom OpenAI-compatible coding model.",
    "thinking_capability": "unsupported",
    "context_window": 200000,
    "effective_context_window_percent": 95,
    "max_tokens": 4096,
    "input_modalities": ["text"],
    "base_instructions": "You are Devo, a coding agent. Help the user edit and understand code."
  }
]
```

然後從 model binding 中引用該 `slug`：

```toml
[model_bindings.my-coding-model-example]
enabled = true
model_slug = "my-coding-model"
provider = "my.provider"
request_model = "provider-specific-model-name"
display_name = "My Coding Model"
invocation_method = "openai_chat_completions"
```
