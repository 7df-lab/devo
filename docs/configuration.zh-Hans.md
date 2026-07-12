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
model_name = "deepseek-v4-flash"
display_name = "DeepSeek V4 Flash"
invocation_method = "openai_chat_completions"
default_reasoning_effort = "high"
```

关键区分如下：

- `model_slug` 从 `models.json` 中选择 Devo 的本地模型元数据。
- `provider` 选择已配置的连接记录。
- `model_name` 是发送到 provider 的特定模型字符串。
- `invocation_method` 选择 provider 协议，例如
  [`openai_chat_completions`](https://developers.openai.com/api/reference/chat-completions/overview)、
  [`openai_responses`](https://developers.openai.com/api/reference/responses/overview)，
  或 [`anthropic_messages`](https://platform.claude.com/docs/en/api/messages)。

## 自定义模型

如果想使用的模型不在内置列表中，请将它添加到 `models.json`，然后通过
`config.toml` 绑定。

用户级模型目录：

- macOS/Linux: `~/.devo/models.json`
- Windows: `C:\Users\yourname\.devo\models.json`

项目级覆盖也可以放在 `<workspace>/.devo/models.json`。
在 `models.json` 中，`provider` 是该模型的默认 wire API 元数据；实际端点仍由
`config.toml` 中的 `provider` 字段选择。
若省略 `base_instructions`，Devo 会回退到内置默认 base instructions；显式写空字符串
（`""`）表示该模型不使用 base instructions。

示例 `models.json` 条目：

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

然后从 model binding 中引用该 `slug`：

```toml
[model_bindings.my-coding-model-example]
enabled = true
model_slug = "my-coding-model"
provider = "my.provider"
model_name = "provider-specific-model-name"
display_name = "My Coding Model"
invocation_method = "openai_chat_completions"
```
