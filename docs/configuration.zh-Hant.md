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

- `model_slug` 按 slug 選擇 Devo 的本地模型中繼資料。
- binding 的 `provider` 選擇一個 `[providers.<id>]` 連線記錄。
- `request_model` 是傳送到 provider 的模型 id。
- `invocation_method` 選擇實際使用的 provider 協議，例如
  [`openai_chat_completions`](https://developers.openai.com/api/reference/chat-completions/overview)、
  [`openai_responses`](https://developers.openai.com/api/reference/responses/overview)，
  或 [`anthropic_messages`](https://platform.claude.com/docs/en/api/messages)。

模型中繼資料也有 `provider` 欄位，它描述模型所需的 wire API；binding 的
`invocation_method` 則選擇執行階段連線，兩者應保持一致。API key 仍保存在
`auth.json` 中，並透過 provider 的 `credential` 參照連線。

## 模型中繼資料與自訂模型

在使用者或工作區 `config.toml` 的 `[model.<slug>]` 下設定模型中繼資料。內建 slug
使用部分覆蓋，未寫欄位保留內建值；新 slug 會建立帶安全預設值的自訂模型，
並應透過 `[providers.<id>]` 和 `[model_bindings.<id>]` 連線。

內建模型部分覆蓋範例：

```toml
[model.qwen3-coder-next]
context_window = 262144
effective_context_window_percent = 90
```

有效上下文視窗的精確公式是
`context_window * effective_context_window_percent / 100`；結果既是模型可用上下文，
也是自動壓縮邊界。完整自訂範例：

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
base_instructions = "You are Devo, a coding agent."
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

可設定中繼資料包括：`display_name` 是 picker 中顯示的模型名稱，`description` 是
面向使用者的說明文字，`channel` 是模型分組標籤；`context_window` 和
`effective_context_window_percent` 決定有效上下文，`max_tokens` 是預設回應輸出上限。
取樣預設值中，`temperature` 控制隨機性，`top_p` 控制 nucleus 機率質量，`top_k`
限制候選 token 數量。`provider` wire API 可為 `openai_chat_completions`、
`openai_responses` 或 `anthropic_messages`。推理中繼資料是型別化的：
`reasoning_capability` 可為
`unsupported`、`toggle`、`{ levels = [...] }` 或
`{ togglewithlevels = [...] }`；`reasoning_implementation` 可為 `disabled`、
`request_parameter` 或型別化 `model_variant` 表。model variant 將邏輯推理選擇映射到
不同的 provider-facing model id、可選有效 effort 和可選額外 request body，而不是修改
同一模型的請求參數；`default_reasoning_effort` 選擇預設推理強度。
`input_modalities` 支援 `text` 和 `image`；`truncation_policy` 選擇 byte 或 token
上限，在超大 tool result 進入模型請求前將其截斷；
`supports_image_detail_original` 啟用原始影像細節。

省略 `base_instructions` 時，內建模型保留內建 instructions，自訂模型使用 Devo
預設 instructions；明確空字串（`base_instructions = ""`）表示不使用 base instructions。

舊的 `model = "slug"` 純量仍可讀取。但 `[model.<slug>]` 現在占用頂層 `model`
表命名空間，因此新設定必須透過 `[defaults].model_binding` 選擇作用中連線。

### 從 `models.json` 遷移

舊的 `~/.devo/models.json` 和 `<workspace>/.devo/models.json` 會被忽略。
請手動把仍需使用的欄位複製到使用者或工作區 `config.toml` 的 `[model.<slug>]`
段，並新增或保留對應 provider 和 model binding。API key 繼續放在 `auth.json`，
透過 `[providers.<id>].credential` 參照。
