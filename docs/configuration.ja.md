# 設定

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` が推奨されるセットアップ方法です。手動で設定する場合、Devo は次の順序で設定をマージします:

1. 組み込みデフォルト
2. `DEVO_HOME/config.toml` - ユーザーレベル設定。デフォルトでは macOS/Linux で
   `~/.devo/config.toml`、Windows で `C:\Users\yourname\.devo\config.toml`
3. `<workspace>/.devo/config.toml` - プロジェクトレベル設定
4. CLI flags

認証情報は `DEVO_HOME/auth.json` に分離して保存されます。
`config.toml` には API key を直接保存せず、credential id を参照させてください。

最小構成:

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

重要な分離は次のとおりです:

- `model_slug` は slug で Devo のローカルモデルメタデータを選択します。
- binding の `provider` は `[providers.<id>]` 接続レコードを選択します。
- `request_model` はプロバイダーへ送信されるモデル id です。
- `invocation_method` は実際に使うプロバイダープロトコルを選択します。例:
  [`openai_chat_completions`](https://developers.openai.com/api/reference/chat-completions/overview)、
  [`openai_responses`](https://developers.openai.com/api/reference/responses/overview)、
  [`anthropic_messages`](https://platform.claude.com/docs/en/api/messages)。

モデルメタデータにも `provider` フィールドがあり、モデルが期待する wire API を
表します。binding の `invocation_method` は実行時の接続方法を選ぶため、両者を一致
させてください。API key は引き続き `auth.json` に保存し、provider の `credential`
参照で接続します。

## モデルメタデータとカスタムモデル

ユーザーまたは workspace の `config.toml` の `[model.<slug>]` で設定します。
組み込み slug は部分上書きで、省略したフィールドは組み込み値を保持します。新しい
slug は安全なデフォルトを持つカスタムモデルを作成し、`[providers.<id>]` と
`[model_bindings.<id>]` の両方で接続します。

組み込みモデルの部分上書き例:

```toml
[model.qwen3-coder-next]
context_window = 262144
effective_context_window_percent = 90
```

有効なコンテキストウィンドウの正確な式は
`context_window * effective_context_window_percent / 100` です。その結果がモデルで
利用可能なコンテキストであり、自動 compaction の境界でもあります。完全な例:

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

設定可能なメタデータには、picker に表示するモデル名 `display_name`、ユーザー向け
説明文 `description`、モデルのグループラベル `channel` があります。
`context_window` と `effective_context_window_percent` は有効なコンテキストを決め、
`max_tokens` は既定の response output 上限です。sampling の既定値では、
`temperature` がランダム性、`top_p` が nucleus probability mass、`top_k` が候補 token
数の上限を制御します。`provider` wire API は `openai_chat_completions`、
`openai_responses`、`anthropic_messages` のいずれかです。
reasoning メタデータは型付きです。`reasoning_capability` は `unsupported`、
`toggle`、`{ levels = [...] }`、`{ togglewithlevels = [...] }`、
`reasoning_implementation` は `disabled`、`request_parameter`、または型付き
`model_variant` table です。model variant は同じモデルの request parameter を変える
代わりに、論理 reasoning selection を別の provider-facing model id、任意の effective
effort、任意の extra request body に対応付けます。`default_reasoning_effort` は既定の
effort を表します。`input_modalities` は `text` と `image`、`truncation_policy` は
大きすぎる tool result を model request に含める前に切り詰める byte または token
上限、`supports_image_detail_original` は original image detail を制御します。

`base_instructions` を省略すると、組み込みモデルは組み込み値を保持し、カスタム
モデルは Devo のデフォルトを使います。明示的な空文字列
（`base_instructions = ""`）は base instructions なしを意味します。

従来の `model = "slug"` scalar は引き続き読み取れます。ただし
`[model.<slug>]` がトップレベルの `model` table namespace を使うため、新しい設定は
`[defaults].model_binding` で有効な接続を選択してください。

### `models.json` からの移行

古い `~/.devo/models.json` と `<workspace>/.devo/models.json` は無視されます。
必要なフィールドをユーザーまたは workspace の `config.toml` の
`[model.<slug>]` に手動でコピーし、対応する provider と model binding を追加または
保持してください。API key は `auth.json` に置き、`[providers.<id>].credential` から
参照します。
