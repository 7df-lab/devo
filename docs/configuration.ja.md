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
model_name = "deepseek-v4-flash"
display_name = "DeepSeek V4 Flash"
invocation_method = "openai_chat_completions"
default_reasoning_effort = "high"
```

重要な分離は次のとおりです:

- `model_slug` は `models.json` から Devo のローカルモデルメタデータを選択します。
- `provider` は設定済みの接続レコードを選択します。
- `model_name` はプロバイダーへ送信される、そのプロバイダー固有のモデル文字列です。
- `invocation_method` はプロバイダープロトコルを選択します。例:
  [`openai_chat_completions`](https://developers.openai.com/api/reference/chat-completions/overview)、
  [`openai_responses`](https://developers.openai.com/api/reference/responses/overview)、
  [`anthropic_messages`](https://platform.claude.com/docs/en/api/messages)。

## カスタムモデル

使いたいモデルが組み込みリストにない場合は、`models.json` に追加してから
`config.toml` でバインドします。

ユーザーレベルのモデルカタログ:

- macOS/Linux: `~/.devo/models.json`
- Windows: `C:\Users\yourname\.devo\models.json`

プロジェクトレベルの上書きは `<workspace>/.devo/models.json` に配置できます。
`models.json` の `provider` は、そのモデルのデフォルト wire API メタデータです。
実際のエンドポイントは引き続き `config.toml` の `provider` フィールドで選択されます。
`base_instructions` を省略した場合、Devo は組み込みのデフォルト base instructions に
フォールバックします。明示的な空文字列（`""`）は、そのモデルに base instructions が
ないことを意味します。

`models.json` エントリの例:

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

次に、その `slug` を model binding から参照します:

```toml
[model_bindings.my-coding-model-example]
enabled = true
model_slug = "my-coding-model"
provider = "my.provider"
model_name = "provider-specific-model-name"
display_name = "My Coding Model"
invocation_method = "openai_chat_completions"
```
