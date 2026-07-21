# 配置

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` 是推荐的设置路径。如需手动配置，Devo 会按以下顺序合并设置：

1. 内置默认值
2. `DEVO_HOME/config.toml` - 用户级配置，默认在 macOS/Linux 上为
   `~/.devo/config.toml`，在 Windows 上为 `C:\Users\yourname\.devo\config.toml`
3. `<workspace>/.devo/config.toml` - 项目级配置
4. CLI flags

凭据单独保存在 `DEVO_HOME/auth.json`；`config.toml` 应引用 credential id，
而不是直接存储 API key。

最小结构：

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

关键区分如下：

- `model_slug` 按 slug 选择 Devo 的本地模型元数据。
- binding 的 `provider` 选择一个 `[providers.<id>]` 连接记录。
- `request_model` 是发送到 provider 的模型 id。
- `invocation_method` 选择实际使用的 provider 协议，例如
  [`openai_chat_completions`](https://developers.openai.com/api/reference/chat-completions/overview)、
  [`openai_responses`](https://developers.openai.com/api/reference/responses/overview)，
  或 [`anthropic_messages`](https://platform.claude.com/docs/en/api/messages)。

模型元数据也有 `provider` 字段，它描述模型所需的 wire API；binding 的
`invocation_method` 则选择运行时连接，二者应保持一致。API key 仍保存在
`auth.json` 中，并通过 provider 的 `credential` 引用连接。

## 模型元数据与自定义模型

在用户或工作区 `config.toml` 的 `[model.<slug>]` 下配置模型元数据。内置 slug
使用部分覆盖，未写字段保留内置值；新 slug 会创建带安全默认值的自定义模型，
并应通过 `[providers.<id>]` 和 `[model_bindings.<id>]` 连接。

内置模型部分覆盖示例：

```toml
[model.qwen3-coder-next]
context_window = 262144
effective_context_window_percent = 90
```

有效上下文窗口的精确公式是
`context_window * effective_context_window_percent / 100`；结果既是模型可用上下文，
也是自动压缩边界。完整自定义示例：

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

可配置元数据包括：`display_name` 是 picker 中显示的模型名，`description` 是面向
用户的说明文字，`channel` 是模型分组标签；`context_window` 和
`effective_context_window_percent` 决定有效上下文，`max_tokens` 是默认响应输出上限。
采样默认值中，`temperature` 控制随机性，`top_p` 控制 nucleus 概率质量，`top_k`
限制候选 token 数量。`provider` wire API 可为 `openai_chat_completions`、
`openai_responses` 或 `anthropic_messages`。推理元数据是类型化的：
`reasoning_capability` 可为
`unsupported`、`toggle`、`{ levels = [...] }` 或
`{ togglewithlevels = [...] }`；`reasoning_implementation` 可为 `disabled`、
`request_parameter` 或类型化 `model_variant` 表。model variant 将逻辑推理选择映射到
不同的 provider-facing model id、可选有效 effort 和可选额外 request body，而不是修改
同一模型的请求参数；`default_reasoning_effort` 选择默认推理强度。
`input_modalities` 支持 `text` 和 `image`；`truncation_policy` 选择 byte 或 token
上限，在超大 tool result 进入模型请求前将其截断；
`supports_image_detail_original` 启用原始图像细节。

省略 `base_instructions` 时，内置模型保留内置 instructions，自定义模型使用 Devo
默认 instructions；显式空字符串（`base_instructions = ""`）表示不使用 base instructions。

旧的 `model = "slug"` 标量仍可读取。但 `[model.<slug>]` 现在占用顶层 `model`
表命名空间，因此新配置必须通过 `[defaults].model_binding` 选择活动连接。

### 从 `models.json` 迁移

旧的 `~/.devo/models.json` 和 `<workspace>/.devo/models.json` 会被忽略。
请手动把仍需使用的字段复制到用户或工作区 `config.toml` 的 `[model.<slug>]`
段，并添加或保留对应 provider 和 model binding。API key 继续放在 `auth.json`，
通过 `[providers.<id>].credential` 引用。
