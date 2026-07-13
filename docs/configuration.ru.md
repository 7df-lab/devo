# Конфигурация

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` - рекомендуемый путь настройки. Для ручной конфигурации Devo
объединяет настройки в таком порядке:

1. Встроенные значения по умолчанию
2. `DEVO_HOME/config.toml` - пользовательская конфигурация, по умолчанию
   `~/.devo/config.toml` на macOS/Linux и
   `C:\Users\yourname\.devo\config.toml` на Windows
3. `<workspace>/.devo/config.toml` - конфигурация уровня проекта
4. CLI flags

Учетные данные хранятся отдельно в `DEVO_HOME/auth.json`; `config.toml` должен
ссылаться на credential id, а не хранить API key напрямую.

Минимальная структура:

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

Важное разделение:

- `model_slug` выбирает локальные метаданные модели Devo из `models.json`.
- `provider` выбирает настроенную запись подключения.
- `request_model` - строка модели, специфичная для поставщика и отправляемая по wire.
- `invocation_method` выбирает протокол поставщика, например
  [`openai_chat_completions`](https://developers.openai.com/api/reference/chat-completions/overview),
  [`openai_responses`](https://developers.openai.com/api/reference/responses/overview)
  или [`anthropic_messages`](https://platform.claude.com/docs/en/api/messages).

## Пользовательские модели

Если нужной модели нет во встроенном списке, добавьте ее в `models.json`, затем
привяжите через `config.toml`.

Пользовательский каталог моделей:

- macOS/Linux: `~/.devo/models.json`
- Windows: `C:\Users\yourname\.devo\models.json`

Переопределения уровня проекта также можно поместить в
`<workspace>/.devo/models.json`. В `models.json` поле `provider` является
метаданными wire API по умолчанию для модели; фактический endpoint по-прежнему
выбирается полем `provider` в `config.toml`.
Если `base_instructions` опущено, Devo использует встроенные base instructions по
умолчанию. Явная пустая строка (`""`) означает, что у модели нет base instructions.

Пример записи `models.json`:

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

Затем сошлитесь на этот `slug` из model binding:

```toml
[model_bindings.my-coding-model-example]
enabled = true
model_slug = "my-coding-model"
provider = "my.provider"
request_model = "provider-specific-model-name"
display_name = "My Coding Model"
invocation_method = "openai_chat_completions"
```
