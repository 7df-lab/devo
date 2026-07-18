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

- `model_slug` выбирает локальные метаданные модели Devo по slug.
- `provider` в binding выбирает запись подключения `[providers.<id>]`.
- `request_model` - id модели, отправляемый поставщику.
- `invocation_method` выбирает рабочий протокол поставщика, например
  [`openai_chat_completions`](https://developers.openai.com/api/reference/chat-completions/overview),
  [`openai_responses`](https://developers.openai.com/api/reference/responses/overview)
  или [`anthropic_messages`](https://platform.claude.com/docs/en/api/messages).

В метаданных модели тоже есть поле `provider`: оно описывает wire API модели.
`invocation_method` в binding выбирает рабочее подключение; эти значения должны
соответствовать друг другу. API key остается в `auth.json` и подключается через
ссылку `credential` поставщика.

## Метаданные и пользовательские модели

Настройте метаданные в пользовательском или workspace `config.toml` в разделе
`[model.<slug>]`. Для встроенного slug это частичное переопределение: пропущенные
поля сохраняют встроенные значения. Новый slug создает модель с безопасными
значениями по умолчанию; подключите ее через `[providers.<id>]` и
`[model_bindings.<id>]`.

Пример частичного переопределения встроенной модели:

```toml
[model.qwen3-coder-next]
context_window = 262144
effective_context_window_percent = 90
```

Точная формула эффективного контекстного окна:
`context_window * effective_context_window_percent / 100`; результат является
доступным модели контекстом и границей автоматической compaction. Полный пример:

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

Настраиваемые поля: `display_name` - имя модели в picker, `description` - пояснение
для пользователя, `channel` - метка группировки моделей. `context_window` и
`effective_context_window_percent` определяют эффективный контекст, а `max_tokens`
является лимитом response output по умолчанию. Среди sampling-настроек
`temperature` управляет случайностью, `top_p` - nucleus probability mass, а
`top_k` - числом token-кандидатов. Wire API `provider` принимает
`openai_chat_completions`, `openai_responses` или `anthropic_messages`.
Reasoning-метаданные типизированы:
`reasoning_capability` может быть `unsupported`, `toggle`, `{ levels = [...] }`
или `{ togglewithlevels = [...] }`; `reasoning_implementation` - `disabled`,
`request_parameter` или типизированная таблица `model_variant`. Model variant
сопоставляет логический reasoning selection с другим provider-facing model id,
необязательным effective effort и extra request body вместо изменения параметра
того же model; `default_reasoning_effort` задает effort по умолчанию.
`input_modalities` принимает `text` и `image`, `truncation_policy` выбирает лимит
bytes или tokens для усечения слишком большого tool result перед включением в
model request, а `supports_image_detail_original` включает исходную детализацию.

Если `base_instructions` пропущено, встроенная модель сохраняет встроенное
значение, а пользовательская использует инструкции Devo по умолчанию. Явная
пустая строка (`base_instructions = ""`) означает отсутствие base instructions.

Старый scalar `model = "slug"` по-прежнему читается. Но `[model.<slug>]` теперь
занимает namespace таблицы верхнего уровня `model`, поэтому новая конфигурация
должна выбирать подключение через `[defaults].model_binding`.

### Переход с `models.json`

Старые `~/.devo/models.json` и `<workspace>/.devo/models.json` игнорируются.
Вручную скопируйте нужные поля в `[model.<slug>]` пользовательского или workspace
`config.toml`, затем добавьте или сохраните соответствующие provider и model
binding. API key храните в `auth.json` и ссылайтесь на него через
`[providers.<id>].credential`.
