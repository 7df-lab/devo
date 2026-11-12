# 本地与局域网模型配置

本文面向 Devo Desktop 和产品 TUI（`devo`），以 **main `1d2d46f`** 的代码为基线（2026-11-12）。旧版发行包可能没有自定义模型端点、`request_model`、`thinking_level_map` 或独立自定义目录，请先用 `devo --version` 核对版本；这里不是对所有历史版本的承诺。

Devo 是模型服务的客户端，不负责下载权重、启动推理引擎或改变引擎的上下文上限。先用 Ollama、LM Studio、vLLM、llama.cpp 或其他服务启动兼容 HTTP API，再配置 Devo。服务名不决定协议；以实际暴露的接口为准。

## 1. 最短可用路径

### Desktop

1. 打开 Settings → Models & providers，进入连接/Providers 页面。
2. 使用 **Add Custom Provider** 创建未使用过的 connection ID，例如 `local`；已有连接用 Edit，不能再添加同名连接。
3. 填入服务的 Base URL 和协议。普通 OpenAI 兼容服务先选 Chat Completions。没有鉴权的 OpenAI 兼容服务可留空 API key。
4. 保存连接，使用 **Discover** 查找模型，或手动添加模型。填服务真实的 Model ID，不是任意显示名称。
5. 检查模型的上下文、输出上限、推理能力和输入模态，再保存并在聊天模型选择器中选中它。
6. 发一条短消息，再测试一次工具调用。目录发现成功不等于模型可以正常生成或调用工具。

并非所有 JSON 高级字段都有等价的 UI 输入控件。需要本篇的高级配置时可编辑文件；编辑前备份，并在重启 Devo 后验证。不要在 UI 保存的同时手工写文件。

### TUI

启动 `devo` 后用 `/login` 连接 provider，用 `/model` 选模型，`/effort` 选推理强度。也可以先按下面的文件格式配置，再启动 TUI。Desktop 和 TUI 的产品请求都走 Native server；不要把 `apps/tui/lib/` 中上游 pi 的独立 `models.json` 教程误当作 Devo 的配置入口。

## 2. 文件位置、覆盖关系与默认选择

默认配置根目录是：

- macOS/Linux：`~/.devo`
- Windows：用户主目录下的 `.devo`，通常是 `%USERPROFILE%\.devo`
- 设置 `DEVO_HOME` 可更换根目录，但该目录必须预先存在。Desktop 从桌面启动时不一定继承终端环境变量。

| 文件 | 用途 |
| --- | --- |
| `<DEVO_HOME>/providers.json` | 已连接的目录 provider 及连接覆盖项 |
| `<DEVO_HOME>/custom-providers.json` | 自定义 provider/model，推荐手写本地模型放这里 |
| `<DEVO_HOME>/auth.json` | 用户级凭据，不放进仓库 |
| `<DEVO_HOME>/config.toml` | 会话默认模型、应用设置、代理和目录策略 |
| `<workspace>/.devo/providers.json` | 工作区 provider 覆盖 |
| `<workspace>/.devo/custom-providers.json` | 工作区自定义 provider 覆盖 |
| `<workspace>/.devo/config.toml` | 工作区应用设置覆盖 |

两个 JSON 目录文件使用同一 schema。当前加载顺序是用户 TOML → 用户 providers → 用户 custom-providers → 工作区 TOML → 工作区 providers → 工作区 custom-providers → CLI 覆盖；目录模型再与内置/远端目录元数据合并。相同 provider/model 的部分字段可以覆盖，不必复制整个目录。provider 内还有 `model_overrides`，用于显式元数据覆盖。不要在多个文件重复维护同一连接，避免“改了没生效”。

UI 写入用户级文件：已有记录尽量保留原文件，新自定义连接写入 `custom-providers.json`；目录模板连接写入 `providers.json`。工作区覆盖仍可能改变有效结果。发现服务要求先有用户 Connection，因此不要只放工作区文件就期待 Discover 可用。

默认选择推荐写在 `config.toml` 顶层（已有内容应合并，不是覆盖整个文件）：

```toml
model = "local/my-model"
small_model = "local/my-model"
model_reasoning_effort_selection = "off"
```

`small_model` 用于轻量后台工作，示例显式固定为同一本地模型，以免误用其他服务。JSON 顶层 `model`、`small_model`、`reasoning_effort` 仍可读取，但当前用户默认选择会迁移/写回 `config.toml`，不要依赖它们长期留在 JSON。已有会话也可能保留自己的模型设置，改变全局默认不等于改变每个已有会话。

## 3. 无密钥本机配置

将以下内容合并到 `<DEVO_HOME>/custom-providers.json`。把 `my-model` 换成服务返回的 ID；数值只是示例，不代表你的模型真实能力。

```json
{
  "provider": {
    "local": {
      "name": "Local model server",
      "base_url": "http://127.0.0.1:1234/v1",
      "wire_api": "openai_chat_completions",
      "enabled": true,
      "models": {
        "my-model": {
          "origin": "user",
          "name": "My local model",
          "context_window": 32768,
          "effective_context_window_percent": 90,
          "max_tokens": 4096,
          "input_modalities": ["text"],
          "reasoning_capability": "unsupported"
        }
      }
    }
  }
}
```

`origin: "user"` 标明手工模型，便于 UI 按用户自有记录管理；没有 provenance 的条目会被保守视为只读。`name` 仅是显示名；默认请求模型 ID 就是 `models` 下的键。ID 可包含 `/`，完整选择是 `provider/model-id`，例如 `local/org/model`。

常见地址示例（以服务实际监听端口为准）：

| 服务 | Base URL 示例 | 注意 |
| --- | --- | --- |
| Ollama | `http://127.0.0.1:11434/v1` | 使用 OpenAI 兼容接口；保留真实 tag，如 `qwen3:8b` |
| LM Studio | `http://127.0.0.1:1234/v1` | 先启动服务并加载模型 |
| vLLM | `http://127.0.0.1:8000/v1` | ID 通常取决于 served model name |
| llama.cpp server | `http://127.0.0.1:8080/v1` | 端口、alias、工具模板由服务端决定 |
| 局域网服务器 | `http://192.168.1.50:8000/v1` | 需要监听可访问网卡并允许对应客户端访问 |

上表不是 Devo 的内置服务预设保证，也不表示所有版本都完整支持 streaming、tools、vision 或 reasoning。

## 4. 协议与端点

`wire_api` 当前支持四种值，模型级值优先于 provider 级值：

| 值 | Base URL 示例 | 最终请求路径 |
| --- | --- | --- |
| `openai_chat_completions` | `http://127.0.0.1:8000/v1` | `/v1/chat/completions` |
| `openai_responses` | `http://127.0.0.1:8000/v1` | `/v1/responses` |
| `anthropic_messages` | `http://127.0.0.1:8000` | `/v1/messages` |
| `google_generative_ai` | Google 兼容 API 根路径 | `…/models/{model}:generateContent` |

前三种常用于本地兼容网关；Google 协议也存在，但不是把普通 OpenAI 服务器切到 Google 就能工作。虽然目录投影在缺省时会显示 Chat Completions，但 Native 连接要求固定协议；如果 provider 和 model 都没写 `wire_api`，运行时会报 no pinned protocol。手写可用连接必须在至少一处明确填写。

- OpenAI 系列裸主机地址会补 `/v1`；显式非根路径保留。因此 `http://host:8000/api` 会请求 `/api/chat/completions`，不会自动变成 `/api/v1/chat/completions`。
- Anthropic 的 adapter 添加 `/v1/messages`；构建时会移除 Base URL 末尾已有的 `/v1`，防止重复。
- 不要将完整 `/chat/completions`、`/responses` 或 `/messages` 路径写成 Base URL，否则会再次追加。
- 使用绝对 `http://` 或 `https://` URL；不要填裸 IP、模型文件路径或 `0.0.0.0`。后者是监听地址，不是客户端目的地址。
- 模型可有自己的 `base_url`、`wire_api`、`request_model`。适合一个连接下多个网关路由；凭据仍来自该 provider，因此不同信任域应建不同连接，避免把同一凭据送到其他主机。

模型级别别名示例，下面是单个 model 对象而非整个文件：

```json
{
  "origin": "user",
  "name": "Coding model",
  "base_url": "http://127.0.0.1:8000/v1",
  "request_model": "served-coder",
  "wire_api": "openai_chat_completions",
  "context_window": 32768,
  "max_tokens": 4096
}
```

当前代码确实支持 `request_model`，不要照搬旧文档里“不存在别名字段”的说法。

## 5. 局域网、容器、代理与安全

请求从运行 **devo-server** 的主机发起。`localhost` 指那台机器，不一定是你眼前的 Desktop、SSH 终端或宿主机。容器、WSL、虚拟机各有自己的网络命名空间；容器访问宿主机地址取决于容器平台，不保证 `host.docker.internal` 到处可用。

检查服务监听地址、端口映射、防火墙和路由。只允许可信客户端访问；本机服务优先绑定回环地址。跨机器传输优先使用正确证书的 HTTPS 或可信隧道。HTTP 会明文传输提示词、代码和可能的密钥，不应暴露到公网。不要通过关闭 TLS 校验来解决证书问题。

无密钥只对不要求鉴权的 OpenAI Chat/Responses 路径成立。当前 Anthropic 和 Google adapter 要求 API key；即使你的本地服务不校验，也不能假设空 key 可通过 Devo 的适配器构建。优先使用服务支持的 OpenAI 路径，或按网关要求配置凭据。

有鉴权时，provider 增加 `"credential": "lan"`，用户级 `auth.json` 使用当前格式：

```json
{
  "lan": {
    "type": "api_key",
    "key": "REPLACE_WITH_YOUR_SERVER_KEY"
  }
}
```

这是占位符，不可原样用于需要真实鉴权的服务。不要在 provider JSON 中放 `apiKey`、`api_key`，不要把密钥写在 URL、截图、日志或 git 提交里。旧 `version/credentials/kind/value` envelope 会被兼容读取并迁移，不是新配置的推荐格式。限制 `auth.json` 文件访问权限；不要假设明文文件自动加密。

`credential` 是引用，不是环境变量名。当前 Native 运行时不通过 provider `env` 查找 API key，也不会把 `${MY_KEY}` 展开成密钥；它会按保存的值处理。`headers` 支持字符串映射，但在这里写 Authorization 会把秘密混进普通配置；认证优先使用 `auth.json`。

应用级代理可在 `config.toml` 配置：

```toml
[provider_http]
proxy_url = "http://127.0.0.1:7890"
no_proxy = "localhost,127.0.0.1,192.168.1.50"
```

这是生成请求的 transport 配置，不是模型字段。模型发现单独创建 HTTP client，不能假定它完全复用 `[provider_http]`。因此“聊天可以但 Discover 失败”也应检查系统代理环境；反之亦然。

本地模型不等于全应用离线：远端模型目录、联网工具、更新检查等仍可能联网。只控制目录时可用：

```toml
[catalog]
offline = true
refresh_on_startup = false
```

`catalog.offline = true` 也会禁止 `provider/discover`，包括本机 Discover；已保存的本地连接仍可用。此时手工填写模型。严格隔离环境还需单独审查应用工具、MCP、更新和网络策略；这两个开关不是全局网络防火墙。

## 6. 模型元数据与 token 预算

| 字段 | 类型/作用 |
| --- | --- |
| `context_window` | 正整数 token 上限；必须按服务实际加载的上下文配置，不是只看模型宣传值 |
| `effective_context_window_percent` | 数值百分比，控制有效上下文和自动压缩边界；建议留余量 |
| `max_tokens` | 输出 token 默认上限，不是输入长度，也不会让服务器自动扩容 |
| `temperature`、`top_p`、`top_k` | 数值采样默认值；服务或协议不一定支持每项 |
| `input_modalities` | `text` 和/或 `image`，仅声明能力，不能把文本模型变成视觉模型 |
| `supports_image_detail_original` | 原始图片细节能力提示，仅在对应 adapter 支持时有意义 |
| `truncation_policy` | `{"mode":"tokens","limit":8000}` 或 `{"mode":"bytes","limit":32000}`，不是服务上下文长度 |
| `base_instructions` | 模型基础指令覆盖；显式空字符串不等同于省略 |
| `enabled`、`priority` | 可用性开关、隐式默认选择优先级 |
| `name`、`channel`、`family`、`release_date`、`status` | 显示、分组或目录元数据 |
| `capabilities`、`cost`、`metadata` | 开放 JSON 元数据，不能保证 adapter 自动实现其中任意能力 |

有效上下文约为 `context_window × effective_context_window_percent / 100`。实际输入还包括系统指令、历史、工具定义/结果和图片，输出也占服务预算。服务启动时只有 8192 上下文，Devo 写 131072 不会改变这个限制。自定义目录模型省略上下文时当前会回落到 200000，这不是自动探测结果，对小模型尤其应显式填写。

工具支持应做实际验证：编码代理依赖正确的 tool-call schema、调用 ID、参数 JSON、流式结束语义及工具结果回传。模型可生成普通文本不表示这些都可用。服务端需要正确 chat template 或工具 parser 时，请按该服务的对应版本配置。

## 7. 推理、档位与 wire 映射

把“模型能否推理”“界面可选档位”“服务请求参数”分开配置。Devo 不能通过配置让不支持推理的模型获得推理能力。

- `reasoning_capability: "unsupported"`：不提供推理选择。
- `reasoning_capability: "toggle"`：开/关，逻辑值 `on`、`off`。
- `reasoning_capability: {"levels":["off","low","medium","high"]}`：枚举选择。含 `off` 才表示可关闭；不含则不能假定能关闭。
- `default_reasoning_selection`：精确默认选择，例如 `off`、`on` 或某个档位；`default_reasoning_effort` 是旧式 effort 默认字段。
- `reasoning`、`thinking_level_map`：pi 兼容的可配置推理描述；不要与 capability 填互相矛盾的值。
- `reasoning_implementation`：兼容字段，旧 TOML/模型 variant 迁移会遇到；新配置优先使用能力、map 和命名 `variants`，而非复制旧 rule 数组。

### 固定档位与服务值不是同一回事

`thinking_level_map` 的逻辑键只能是 `off`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max`；值只能是字符串或 `null`。不能增加 `fast`、`deep`、`ultra` 等任意 UI 档位。`null` 表示该逻辑档不支持，不是“发送 null”。底层 effort 枚举还接受 `none`，能力投影会将其归一化为 UI 的 `off`，但它不是 map 键。

例如服务实际接受 `none`、`small`、`large` 三种字符串时，可在模型对象中配置：

```json
{
  "reasoning": true,
  "reasoning_capability": {"levels": ["off", "low", "high"]},
  "default_reasoning_selection": "low",
  "thinking_level_map": {
    "off": "none",
    "minimal": null,
    "low": "small",
    "medium": null,
    "high": "large",
    "xhigh": null,
    "max": null
  }
}
```

`small`/`large` 只是展示映射机制，不是可直接套用的通用协议值。map 主要产生 `request_thinking`，默认 OpenAI Chat adapter 仍使用 typed effort，不保证将这些任意字符串发送为 `reasoning_effort`。需要确保请求出现精确自定义参数时，使用下面的 variant request。map 不支持数字预算，不能写 `"high": 8192`。具体 adapter/profile 决定 wire 行为，不能只凭 UI 显示档位就断定服务器收到或接受它。

缺失的 map key 不等于不支持：`off/minimal/low/medium/high` 缺省开放，`xhigh/max` 需要显式非 null 才开放。所以只写 `{"high":"high"}` 不会只显示 High；非空 map 优先于 capability 自动生成的 map，不与之深合并。请像示例一样明确关闭不支持的档位。

### 显式请求体 variant

服务器用自定义开关而非标准 effort 时，可使用命名 `variants`。例如**仅当服务支持** `chat_template_kwargs.enable_thinking` 时，在模型对象配置：

```json
{
  "reasoning_capability": {"levels": ["off", "high"]},
  "default_reasoning_selection": "off",
  "variants": {
    "off": {
      "label": "No thinking",
      "request": {"chat_template_kwargs": {"enable_thinking": false}}
    },
    "high": {
      "label": "Thinking",
      "request": {"chat_template_kwargs": {"enable_thinking": true}}
    }
  }
}
```

匹配 effort variant 后，runtime 会清空通用 thinking/effort 选择，但 adapter/profile 仍可能合成默认 wire 字段（例如部分 GLM/DeepSeek 路径默认启用 thinking）。因此 variant 必须包含服务需要的完整控制字段，必要时显式覆盖 adapter 的默认值，并检查实际请求。

这不是 Ollama、vLLM 或所有 Qwen 版本通用保证。若服务要求 `thinking`、`reasoning`、`reasoning_effort` 或 token budget，应按其接口文档修改 `request`，并验证实际请求。与推理档位匹配的 `variants.<effort>.request_model` 还可切换服务模型 ID；这仍在同一个 provider 的连接/凭据范围内。

命名 variant 支持 `label`、`disabled`、`request_model`、`request`、`options`、`headers`；`default_variant` 可指定默认项。variant 可以用业务名称，但不要混淆为新增 `/effort` 档位。当前任意业务 variant（例如 `fast`）或 `default_variant` 的 `request_model` 并不保证参与最终模型路由；其 request/options/headers 可被合并，wire 模型却仍可能使用基础 ID。业务别名需切换模型 ID 时，推荐建独立模型条目并使用模型级 `request_model`。完整引用可为 `provider/model/variant`，精确模型 ID 优先匹配，所以模型 ID 与 variant 后缀冲突时需要避免歧义。

## 8. 高级请求选项与覆盖规则

provider 级支持 `name`、`description`、`base_url`、`credential`、`headers`、`wire_api`、`enabled`、`env`、`request`、`options`、`compat`、`web_search`、`web_fetch`、`models` 和 `model_overrides`。模型字段见上表及推理章节，额外支持 `base_url`、`request_model`、`origin`、`headers`、`request`、`options`、`variants`、`default_variant`、模型级 web 开关。

`request`、`options` 和 `compat` 是开放结构，不是任意名字都会变成 Devo 功能。当前 Native 请求配置会把 `options` 和 `request` 合入额外请求体；不要凭 SDK 的习惯把 `options.timeout_ms` 当作受支持的客户端超时配置。

合并顺序为 provider → model → variant，每层先 `options` 再 `request`，对象内部递归合并；headers 同名项由更具体层覆盖。随后额外请求体对 adapter 生成的 body 做顶层覆盖，因此嵌套 `reasoning` 对象可能替换 adapter 自己生成的整个对象。`request` 直接放最终 body 字段，不需要包一层 `extra_body`，除非服务确实要求这个字段。避免在透传里随意写 `messages`、`tools`、`model`、`stream`、认证等核心字段。

本地服务通常不支持 provider-hosted web tools。provider/model 中的 `"web_search":{"mode":"disabled"}`、`"web_fetch":{"mode":"disabled"}` 会关闭对应 web 能力，连同 Devo 本地搜索/抓取，而不只是禁止 hosted API。若要保留 Devo 自己的工具，选择 `mode: "local"`；本地搜索还需配置支持的搜索服务及该服务要求的凭据。web_fetch 的应用默认值就是 local，不应为了“不发 hosted 参数”而无意禁用它。

## 9. 发现、验证与排错顺序

### Discover 的边界

`provider/discover` 必须有已连接的用户 Connection 和明确端点。它只返回临时选择，不把远端目录自动写入配置；选择模型并显式保存才成为持久记录。目录缓存约 10 分钟，支持强制刷新。单次发现 HTTP client 超时为 15 秒且不跟随重定向。

通常尝试 `{base}/models`，必要时 `{base}/v1/models`。Ollama ID 的连接在 Base URL 以 `/v1` 结尾时优先 `/api/tags`；其他符合条件的连接也有 tags fallback。响应支持 `data` 或 `models` 数组，并归一化常见字段。服务只返回模型名时，上下文和推理能力仍需要人工核实；远端元数据不是可信的能力保证。

`provider/validate` 会向指定模型发送短生成请求（“Reply with OK only.”），可能消耗服务资源。它不是完整兼容性测试，也不完整覆盖每个自定义 variant 的请求参数。保存后用实际聊天、流式输出、工具调用和推理切换复测。

### 从运行 server 的机器检查

无鉴权 OpenAI 兼容服务可用：

```sh
curl --fail --silent --show-error http://127.0.0.1:8000/v1/models
curl --fail --silent --show-error http://127.0.0.1:8000/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"my-model","messages":[{"role":"user","content":"Reply OK"}],"max_tokens":32,"stream":false}'
```

替换地址和模型 ID。Windows PowerShell 中建议明确调用 `curl.exe` 并按 shell 调整 JSON 引号。有鉴权服务应通过安全方式提供 header，不把真实密钥放在可分享命令或 shell 历史里。此检查证明基本 HTTP 路径，不能替代 Devo 的流式/工具测试。

运行 `devo doctor` 检查配置目录、依赖及 provider readiness。启动诊断可能触发兼容格式迁移，所以改旧配置前先备份。手工改文件后重启相关 Desktop/TUI/server，并在新会话重新选择模型；一个 DEVO_HOME 可能由共享 daemon 管理，单纯关闭一个客户端未必重启服务。

| 现象 | 优先检查 |
| --- | --- |
| Connection refused / timeout | 服务是否启动、监听网卡、容器/WSL 网络、端口、防火墙、代理、server 所在主机 |
| 404 | base URL 是否重复 `/v1` 或完整方法路径、是否选错协议、model/tag 是否真实存在 |
| 401 / 403 | credential 引用、auth.json 当前格式、网关要求的认证方案；不要把变量名字当密钥 |
| Discover 失败但聊天正常 | `/models` 是否提供、catalog.offline、15 秒超时、独立代理路径、重定向 |
| Discover 成功但聊天失败 | 发现只测目录；检查生成路由、模型加载、SSE、tool schema 和协议能力 |
| 修改没生效 | DEVO_HOME、工作区覆盖、custom-providers 优先级、已有会话选择、共享 daemon |
| 模型只读/不可删除 | origin 是否为 user；远端目录不是用户自有记录，可禁用或添加自定义项 |
| reasoning 参数 400 | 服务支持值、逻辑档位与 map、variant 请求体、服务版本；先移除高级参数测试 |
| context exceeded / OOM | 服务真实上下文、KV cache/显存、历史与工具结果、输出预算；降低声明和 max_tokens |
| 普通文本正常但工具卡住 | 服务模型模板、工具 parser、SSE 结束事件、tool-call ID 和参数 JSON |
| 密钥留空仍报错 | 是否选了要求 key 的 Anthropic/Google；是否仍残留 credential 引用 |

## 10. 升级与代码依据

本篇只恢复本功能文档，不恢复旧设计文档树。后续修改配置 schema、路由或发现行为时应同步更新这里。特别注意，仓库旧 README/网页配置示例可能保留早期 auth envelope、三种 wire API、发现自动持久化或没有 `request_model` 的描述；本篇以以下当前源码为准。

- [目录、模型和 variant schema](../crates/config/src/provider/catalog.rs)
- [配置加载与覆盖](../crates/config/src/app.rs)、[连接写入与 ownership](../crates/config/src/provider_connection.rs)
- [认证读取/迁移](../crates/config/src/provider/auth.rs)、[旧配置迁移](../crates/config/src/provider/migration.rs)
- [配置主目录](../crates/utils/paths/src/home_dir.rs)
- [推理类型](../crates/protocol/src/reasoning_effort.rs)、[模型选择解析](../crates/protocol/src/model.rs)
- [请求覆盖合并](../crates/config/src/provider/request.rs)
- [server 路由、端点与认证](../crates/server/src/provider_config.rs)
- [发现实现](../crates/server/src/runtime/provider_discovery.rs)、[连接验证](../crates/server/src/runtime/provider_api.rs)
- [Desktop 模型设置](../apps/desktop/src/renderer/components/settings/model-edit-dialog.tsx)
- [产品 TUI Native 连接](../apps/tui/src/native-agent-connection.ts)
