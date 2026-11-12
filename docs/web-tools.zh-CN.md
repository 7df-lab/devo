# Web Search / Web Fetch：原生能力、第三方配置与实际执行路径

本文说明 Devo **已经实现的行为**，不把上游服务支持的功能当成 Devo 已经接入的功能。源码基线：`1d2d46fa377c936edd8ad2f6213d87164dd69ac3`（2026-11-12 核对）；官方资料核对日期相同。配置示例中的密钥都是占位符，请合并到已有文件，不要覆盖其他连接或凭据。

## 1. 先选哪条路径

| 需求 | 推荐配置 | 实际执行者 |
| --- | --- | --- |
| 接入点和所选模型明确支持原生搜索 | `web_search.mode = "provider"` | 模型服务端 |
| 接入点没有原生搜索，或需要独立控制搜索服务 | `web_search.mode = "local"`，选择 Exa、Tavily 或 Parallel | Devo 调用第三方服务 |
| 获取一个普通公开 URL | `web_fetch.mode = "local"`（默认） | Devo 直接 HTTP GET |
| 已确认接入点支持 Anthropic 原生抓取 | `web_fetch.mode = "provider"` | 模型服务端 |
| 不提供对应的常规工具入口 | `mode = "disabled"` | 不在该能力的常规入口启用 |

**当前没有 `auto` 模式，也没有“探测 provider 不支持 → 自动切换 Exa/Tavily/Parallel”的逻辑。** 原生工具报错不会触发第三方重试；配置多个搜索服务也不构成轮询或故障转移列表。搜索与抓取独立选择，可以“原生搜索 + 本地抓取”，也可以两者都走本地。

默认值是 **search = provider，fetch = local**。对于 Ollama、LM Studio、vLLM 等只有兼容推理 API 的接入点，通常应显式配置本地搜索，不要把“支持 function calling”理解为“服务端自带联网搜索”。本地搜索中的“本地”指调用代码在 Devo 一侧，**仍会联网发送查询**。

`disabled` 是功能配置，不是断网或安全隔离开关。独立 MCP、技能或其他获准的联网路径不随之关闭；当前 RLM 的 `web.fetch` host bridge 也没有检查 fetch mode，仍执行自身的权限检查。需要禁止联网时，应使用权限和网络隔离策略，不能只依赖这些 mode。

## 2. 名称与调用链

### Provider-hosted

```text
所选 provider/model + 有效 mode
  → TurnConfig
  → ModelRequest.hosted_tools
  → 协议适配器序列化
  → 模型服务端执行
  → hosted tool 事件/结果进入对话
```

原生工具不是交给 Devo 的本地 `web_search` handler 执行。费用、限流、可访问页面、模型支持范围及服务端数据处理规则由接入点决定。工具展示为 completed 也不能代替检查结果正文：服务端可能在正常 HTTP 响应中返回工具级错误。

### Local / RLM

当前正常模型请求只暴露 `ipython` 作为客户端函数工具，hosted tools 另行加入。模型通常通过 Python 使用：

```python
print(await rlm.web_search("Rust 官方发布说明"))
print(await rlm.web_fetch("https://www.rust-lang.org/", format="markdown", timeout=30))
```

对应链路：

```text
rlm.web_search(query) → host_request("web.search", ...) → 权限检查 → WebSearchHandler
rlm.web_fetch(url)    → host_request("web.fetch", ...)  → 权限检查 → WebFetchHandler
```

内部搜索工具规范名为 `web_search`；抓取 handler 名为 `webfetch`。Python 的 `web_fetch` 与 provider 的 `web_fetch` 不是同一个执行端。不要因为直接函数列表没有 `web_search` 就认定搜索不可用，应同时检查 RLM 与 hosted tools。

`[tools] execution_surface = "rlm"` 是默认值；`"discrete"` 不启动 RLM，也不向模型恢复传统独立函数工具列表。内核不可用时不能假定本地 helper 仍可正常调用。

## 3. 配置位置、优先级与继承

- 应用配置：`<DEVO_HOME>/config.toml`，默认 `~/.devo/config.toml`。
- 项目应用覆盖：`<workspace>/.devo/config.toml`。
- provider/model 覆盖：用户或项目的 `providers.json`；自定义连接也可在相应的 `custom-providers.json` 中配置。
- 密钥：**用户级** `<DEVO_HOME>/auth.json`，不要放到项目配置、Git 或对话里。
- `DEVO_HOME` 若设置，必须指向已经存在的目录。

应用 TOML 按用户 → 项目 → CLI 覆盖加载。对每个 web 能力，运行时再按下列优先级选择：

1. 所选模型的 `web_search` / `web_fetch` 对象；
2. 所选 provider 的同名对象；
3. 应用层 `[tools.web_search]` / `[tools.web_fetch]`。

这是**选择整个对象**，不是逐字段继承。比如模型设置 `{"web_search":{"mode":"local"}}` 后，不会自动继承 provider/global 的 `local_provider`。本地服务定义始终从合并后的全局 `tools.web_search.local_providers` 表查询，不能只写在模型对象中。

本地服务选择规则：有效对象指定非空 `local_provider` 时用该 ID；否则仅在全局服务表恰好只有一个条目时自动选择它；零个或多个条目均报配置错误。建议始终写清 ID。

### 按接入点或模型覆盖

将以下片段合并到已有的 provider 配置；`your-provider` 和 `your-model` 要替换为真实目录键，不是显示名称。这不是完整的连接创建示例，不应删除现有 `base_url`、认证和模型定义。

```json
{
  "provider": {
    "your-provider": {
      "web_search": {"mode": "local", "local_provider": "tavily"},
      "web_fetch": {"mode": "local"},
      "models": {
        "your-model": {
          "web_search": {"mode": "disabled"}
        }
      }
    }
  }
}
```

这里该 provider 的其他模型使用 Tavily，指定模型禁用常规搜索入口。切换模型可能切换有效模式；排障时必须核对所选 `provider/model`。手动修改文件后建议重启 Devo 并开新会话验证，不保证所有文件修改都在已有 turn 中即时生效。

## 4. 原生工具的实际支持矩阵

| Wire API | `search = provider` 的正常 turn 行为 | `fetch = provider` 的正常 turn 行为 |
| --- | --- | --- |
| `anthropic_messages` | 添加 `{"type":"web_search_20260209","name":"web_search"}` | 添加 `{"type":"web_fetch_20260209","name":"web_fetch"}` |
| `openai_responses` | 添加 `{"type":"web_search"}` | 当前代码添加 `{"type":"web_fetch"}`；不能视为官方支持保证 |
| `openai_chat_completions` | core 层不添加 hosted search | 当前代码写顶层 `web_fetch_options: {}`；不能视为官方支持保证 |

重要边界：

- **协议兼容不等于能力兼容。** 没有按 endpoint、模型或账户自动探测原生 web 能力，兼容代理可能拒绝、忽略或自行实现这些字段。
- Chat Completions 的底层适配器能序列化 `web_search_options`，但正常 core turn 的 search gate 只允许 Responses 和 Anthropic，因此不能照旧 README 描述期待它自动启用。官方 Chat Completions 搜索需要适配的模型，这与当前 Devo 正常路径是否发送参数是两件事。
- OpenAI 官方 Web Search 文档描述了搜索过程中的 `open_page` / `find_in_page`，不能据此推导出独立 `web_fetch` 工具或 `web_fetch_options` 受支持。对普通 OpenAI 接入点保留 `fetch = local` 更稳妥；只有接入点明确提供相应扩展且经过验证时才使用当前 provider fetch 序列化。
- Anthropic 的版本化工具需与模型、平台及账户匹配。当前 Devo 固定使用上述 20260209 版本，不会自动回退到旧版，也不会自动跟随官方新版。官方动态过滤功能有模型要求；不能仅看 `anthropic_messages` 就认定可用。
- 原生 search 使用模型接入点本身的认证，不使用 Exa/Tavily/Serper 的 key。

最小全局原生配置如下。使用前先确认所选接入点支持；不需要原生 fetch 时改回 `local`。

```toml
[tools.web_search]
mode = "provider"

[tools.web_fetch]
mode = "provider"
```

### 哪些 hosted 参数能配置

用户 web 配置目前只提供 search 的 `mode`、`local_provider`、`local_providers` 与 fetch 的 `mode`。正常 core turn 创建默认 `HostedWebSearchTool::new()` / `HostedWebFetchTool::new()`，没有从这些配置映射域名过滤、地理位置、引用开关、工具版本、`max_uses` 等高级参数。

协议内部结构虽包含 search 的 `search_context_size` / `max_uses` / `anthropic_tool_type`，以及 fetch 的域名列表、`citations`、`max_content_tokens` 等字段，**不代表在 TOML 添上同名字段就生效**。协议审查修复后，Anthropic fetch serializer 已将内部布尔 `citations` 序列化为官方要求的对象 `{"enabled":true}`（禁用时为 `{"enabled":false}`）。默认 core 路径仍未设置它，也未从用户 web 配置开放引用开关，因此不能直接在 TOML 添加 `citations` 来启用引用。

官方依据：[OpenAI Web Search](https://developers.openai.com/api/docs/guides/tools-web-search)、[Anthropic Web Search](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool)、[Anthropic Web Fetch](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-fetch-tool)。具体可用模型、额度和价格请以接入点当前文档与账户为准。

## 5. 内置第三方搜索：Exa / Tavily / Parallel

### 5.1 Exa

`config.toml`：

```toml
[tools.web_search]
mode = "local"
local_provider = "exa"

[tools.web_search.local_providers.exa]
kind = "exa"
credential = "exa"
max_results = 5

[tools.web_fetch]
mode = "local"
```

用户级 `auth.json` 中合并：

```json
{
  "exa": {"type": "api_key", "key": "REPLACE_WITH_EXA_KEY"}
}
```

Devo POST 到 `https://api.exa.ai/search`，以 `x-api-key` 认证；发送 `query`、`type: "auto"`、`numResults` 和 `contents: {"highlights":true}`。结果整理为标题、URL、摘要；摘要优先使用返回的 summary，其次 text，最后第一个 highlight。不会自动读取所有搜索结果页面。

### 5.2 Tavily

`config.toml`：

```toml
[tools.web_search]
mode = "local"
local_provider = "tavily"

[tools.web_search.local_providers.tavily]
kind = "tavily"
credential = "tavily"
max_results = 5
```

用户级 `auth.json`：

```json
{
  "tavily": {"type": "api_key", "key": "REPLACE_WITH_TAVILY_KEY"}
}
```

Devo POST 到 `https://api.tavily.com/search`，使用 `Authorization: Bearer ...`。固定参数包括 `auto_parameters=false`、`topic="general"`、`search_depth="basic"`、`chunks_per_source=3`。`time_range` / `country` 为 null，域名列表为空，answer/raw content/images/image descriptions/favicon/usage 均不请求。当前配置没有开放高级搜索、时间范围、地区、域名限制等开关。返回内容整理为标题、URL、content；如果服务仍返回 answer，formatter 会附加它。

### 5.3 Parallel（匿名 MCP 搜索）

不需要 API key，也不需要另建 `[mcp_servers]` 条目：

```toml
[tools.web_search]
mode = "local"
local_provider = "parallel"

[tools.web_search.local_providers.parallel]
kind = "parallel"
max_results = 5
```

- 默认 endpoint 为 `https://search.parallel.ai/mcp`，使用 Streamable HTTP MCP；每次初始化、列出工具、调用 `web_search`，之后 shutdown。
- 发送 `objective=query`、`search_queries=[query]` 和 Devo 的 `session_id`。匿名不代表不发送会话标识或查询。
- `max_results` 在客户端截取返回列表，不作为 MCP 请求参数发送，因此不控制服务端搜索量或计费。
- 请求必须得到含 results 的结构化 JSON，或文本 content 中的 JSON；错误结果、无 `web_search`、不符合预期的 JSON 都会失败。
- 默认超时 60 秒；若工具上下文带 wall-time budget 则使用该预算，支持取消。`max_results=0` 会报错。
- **必须省略 `credential` 或保持空值**；非空会被配置解析拒绝。此实现不支持以保存的 key 升级到付费/认证模式。
- Parallel 官方现把这个 endpoint 归为 legacy Search MCP，仍为已有用户提供服务；新集成推荐另一个统一 MCP。Devo 此处仍使用旧 endpoint，不能只换 `base_url` 就假定新接口、认证和返回契约完全兼容。
- 官方服务也提供 `web_fetch`，但这个内置 handler **只调用搜索**。`tools.web_fetch.mode="local"` 仍是 Devo HTTP GET，不会改用 Parallel 抓取。

官方依据：[Exa Search](https://exa.ai/docs/reference/search)、[Tavily Search](https://docs.tavily.com/documentation/api-reference/endpoint/search)、[Parallel Search MCP](https://docs.parallel.ai/integrations/mcp/search-mcp)。匿名额度、速率与可用性可能变化，不应把匿名接口当作无限量服务。

### 5.4 共用参数与认证注意事项

| 配置字段 | 含义与限制 |
| --- | --- |
| `kind` | 仅 `exa`、`tavily`、`parallel`；不接受 `serper` 或任意厂商名 |
| `credential` | auth.json 中的条目名，不是 key 本身；Exa/Tavily 必须是 `api_key` 类型 |
| `base_url` | 可省略；覆盖**完整请求 URL**，不会自动追加 `/search` 或 `/mcp` |
| `max_results` | 可省略，默认 5；建议正整数，并遵守上游限额 |

`max_results` 优先使用配置值，其次底层输入值，最后默认 5。公开 `rlm.web_search(query)` helper 只有 query 参数；模型工具 schema 也只声明 query。不要期待模型能用任意高级参数覆盖配置。Exa/Tavily 不在此 handler 中统一检查服务上限或提供专用重试/超时配置；HTTP 错误直接返回，不自动换搜索服务。

当前 auth 文件规范是上面的 provider-keyed `{"exa":{"type":"api_key","key":"..."}}`。旧 `version/credentials/kind/value` envelope 可迁移，但不要新建旧格式；读取时 `_api_key` / `_oauth` 后缀会被规范化移除，而本地搜索 resolver 按 `credential` 原字符串查找。因此请使用一致的 `credential="exa"` 与 auth key `exa`，不要照旧例子写 `exa_api_key`。本地搜索路径不会自动读取 `EXA_API_KEY` / `TAVILY_API_KEY` 环境变量，也不会把 auth 值当作环境变量名或 shell 命令执行；这些变量出现在 live test 中不等于运行时配置支持。

配置多个服务时，可以同时保留不同的 `local_providers.<id>`，通过 `local_provider` 显式选择。不要把模型 API key 当作搜索 key，也不要把不可信镜像地址填入 `base_url`：查询和认证头会发送到该地址。

## 6. 本地 Web Fetch 的能力与限制

只需：

```toml
[tools.web_fetch]
mode = "local"
```

无需 Exa/Tavily 账户。Devo 从运行服务端的机器发起 HTTP GET；远端运行 Devo 时不是用户浏览器发起请求。

| 参数/行为 | 当前实现 |
| --- | --- |
| `url` | 必填，只接受以 `http://` 或 `https://` 开头的 URL |
| `format` | 默认 `markdown`，支持 `text` / `markdown` / `html`；不是浏览器渲染格式 |
| `timeout` | 默认 30 秒，夹在 1–120 秒；建议传整数秒 |
| 大小 | 5 MiB 上限，检查 Content-Length 并在读取后检查实际长度；不是严格的流式内存上限 |
| HTML | 简单文本提取或 Markdown 转换，不执行 JavaScript |
| 图片 | 大部分 image MIME 以 base64 及 MIME 元数据返回；SVG 等有例外，不保证客户端都能直接展示 |
| PDF/其他二进制 | 没有专用 PDF 解析或 OCR，非图片按 UTF-8 lossy 文本处理，不适合作为 PDF 阅读器 |
| 登录/认证 | 不携带用户浏览器 cookie，没有自定义登录、请求头、POST body 或抓取服务 key 配置 |

403、登录墙、验证码、JS-only 页面不会自动交给浏览器或第三方抓取服务。返回成功也应检查正文是否实际上是登录页面。`allowed_domains` 等 hosted 内部字段不约束本地 HTTP fetch；访问限制要在权限/网络层落实。

## 7. Serper 技能回退与通用 MCP

### Serper 是另一条技能路径

bundled `websearch` 技能实现 `websearch.run(...)`，与 `tools.web_search.local_providers` 不是同一种配置：

1. 先尝试 host `web.search`，即当前 turn 的本地搜索配置（包括 Parallel）。函数名称中的 hosted 不表示调用 provider 原生搜索。
2. 只有捕获到“未配置 / needs configuration”类错误，或没有得到可用文本时，才尝试 Serper；一般网络错误、认证错误不构成自动切换条件。
3. 无 key 时给配置提示，不生成伪造结果。

配置入口：TUI `/login` → **MCP Connections** → **Serper (web search)**；它是 service credential，名称出现在 MCP 分类不代表启动了 MCP server。源码在 auth flow 中单独注册了它。也可将以下条目合并到用户 auth.json：

```json
{
  "serper": {"type": "api_key", "key": "REPLACE_WITH_SERPER_KEY"}
}
```

技能调用示例（技能已加载且内核运行）：

```python
print(await websearch.run("Rust 最新发布", max_output=8192, timeout=45, num_results=5))
```

技能 POST 到 `https://google.serper.dev/search`，header 为 `X-API-KEY`，body 为 `{"q": query}`。`num_results` 限制本地输出中的 organic 条目，不控制请求的搜索结果数量；还可能输出 knowledge graph 与最多三个相关问答。`max_output` 默认 8192 字符，超出会截断。

Serper key 查找顺序为 `SERPER_API_KEY` 环境变量，然后 auth.json 的 `serper.type/key`。技能配置目录依次看 `DEVO_CODING_AGENT_DIR`、`DEVO_HOME`、`PI_CODING_AGENT_DIR`，最后 `~/.devo`；这与 Rust host 目录要保持一致。可选环境覆盖 `DEVO_WEBSEARCH_TIMEOUT`（默认 45）和 `DEVO_WEBSEARCH_NUM_RESULTS`（默认 5），显式函数参数优先。一般用户用 `/login` 即可，无需设置环境变量。获取 key 见 [Serper 官网](https://serper.dev)。

技能依赖 bundled skill 与 Python/httpx 可用；禁用或不加载技能时不会凭空出现这条回退。它的 HTTP 客户端不同于 Rust 网络代理客户端，不应假定 `[provider_http]` 会自动套用到 Serper。

### 通用 MCP 可以接其他搜索/抓取服务

对于不在内置三种 kind 内的服务，或者需要独立的 MCP `web_fetch`，可以配置普通 MCP server。它们不会自动替换 `rlm.web_search` / `rlm.web_fetch` 或补齐 provider 原生能力。

以下是**配置形状示例**，URL 要替换为服务商明确提供的 MCP URL，普通 REST `/search` 地址不能直接当作 MCP：

```toml
[mcp_servers.research]
url = "https://your-mcp-service.example/mcp"
type = "streamable_http"
enabled = true

[mcp_servers.research.env_http_headers]
Authorization = "RESEARCH_AUTHORIZATION"
```

环境变量 `RESEARCH_AUTHORIZATION` 的值应为服务要求的完整 header 值（若用 Bearer，则包含 `Bearer ` 前缀）；不要把真实 key 写进项目 TOML。支持静态 `http_headers`，但这会将值放在配置文件中。`auth_ref` 字段当前注明未被 runtime 接线，不能用它代替实际认证配置。Stdio MCP 使用 `command` / `args` / `env` / `env_vars`，不可同时配置 command 和 url。

MCP 工具以自己的服务命名空间注册（例如 `mcp__research__web_search`），由发现结果和权限策略决定能否使用；RLM 有 MCP host helpers。普通 MCP 服务的认证、OAuth 支持、输出限制和收费应按该服务单独核对，不适用匿名 Parallel adapter 的约定。

## 8. 代理、隐私与费用

Rust 本地搜索、抓取及 Parallel adapter 接收 `[provider_http]` 中的代理配置：

```toml
[provider_http]
proxy_url = "http://127.0.0.1:7890"
no_proxy = "localhost,127.0.0.1"
```

显式代理配置优先；未配置时共享客户端读取 `http_proxy` / `HTTP_PROXY`、`https_proxy` / `HTTPS_PROXY`、`all_proxy` / `ALL_PROXY`，以及 `NO_PROXY` / `no_proxy`。具体优先关系见共享代理实现。显式代理时需要的 bypass 应同时写 `no_proxy`。本地代理只决定 Devo 发出请求的路线，不决定模型服务端如何访问网页。

数据流：

- 原生模式：查询/对话与工具执行交给模型接入点及其搜索基础设施。
- 本地搜索：查询和必要认证发给所选服务；Parallel 额外带 session_id。结果进入模型上下文，可能进入会话记录。
- 本地 fetch：目标网站/代理接收 URL、IP 和 HTTP 请求元数据，响应进入模型上下文。
- Serper / MCP：分别遵守各自客户端与服务的数据处理规则。

匿名、read-only 或 local 都不代表离线、免费无限、零留存或无需权限。不要发送秘密、私有 URL token 或不必要的个人数据；不要把认证文件提交到仓库。日志中的 HTTP 错误可能含服务返回正文，分享排障材料前应脱敏。网页、搜索摘要和 MCP 返回内容属于外部不可信数据，不应被当成操作授权或系统指令。

## 9. 验证与排障

修改后从一次新会话开始，要求模型“搜索一个当前公开信息并给来源 URL”，再测试已知公开 URL 的 fetch。分别确认使用的是 provider hosted 事件、RLM host 搜索还是技能/MCP，不能只看最终回答里是否出现链接。

| 现象 | 优先检查 |
| --- | --- |
| provider 模式没有搜索 | 所选 wire_api；Chat Completions 正常 turn 不注入；模型/账户是否支持；更高优先级对象是否 disabled |
| 原生请求 400 / unknown tool | 接入点不支持该类型或工具版本；OpenAI 的独立 fetch 不能按官方能力假定；改用已配置的 local |
| `failed to load web_search credentials` | auth.json 格式、空凭据、读取权限；当前 server 先读整个 auth，失败会禁用搜索，甚至影响 provider 模式 |
| `failed to resolve web_search config` | local_provider 是否存在、是否有多个未选择的服务、key ID 是否匹配、是否错误使用 OAuth |
| `missing credential ...` | 使用当前 auth 格式；避免 `_api_key` 后缀规范化导致查找不一致 |
| `local web_search provider is not configured for this turn` | 当前 turn 不是 local，或配置解析已失败；仅填写 local_providers 不会自动切换 mode |
| Parallel `must omit credential` | 匿名 adapter 不接受 credential；不是 key 写错，需删除该字段或选择另一路径 |
| 401/403/402/429/5xx | 认证、服务权限/余额、限流或服务故障；不会自动切其他厂商；按服务实际响应处理 |
| `invalid JSON` / MCP schema 错误 | base_url 是否完整正确、网关是否返回 HTML、服务是否改了返回协议 |
| fetch 超时/过大/内容乱码 | 网络与代理、1–120 秒超时、5 MiB、JS 页面或不支持的文档格式 |
| 有配置但没有工具可用 | RLM 是否启动、execution_surface 是否 discrete、权限是否拒绝、模型是否支持客户端工具调用 |
| Serper 已登录仍提示配置 | 技能目录与 host 目录是否一致、auth key 是否为 serper、技能/内核是否可用 |

不要通过“隐藏错误后编答案”验证搜索；成功标准包括真实请求路径、可读结果、可核对的来源。没有对所有 provider/model 做付费真实服务测试时，应明确区分源码与 mock 测试覆盖和线上可用性。

## 10. 源码定位与维护清单

本页覆盖配置解析与当前正常 turn，不保证任意第三方网关的扩展协议。旧 crate README / skill 描述可能落后于源码，例如旧 Anthropic 版本、Chat Completions 注入及认证格式。修改 web 行为时应同步本页与这些入口。

- [配置类型、默认值与解析](../crates/config/src/tools.rs)
- [用户凭据读取、迁移和序列化](../crates/config/src/provider/auth.rs)
- [turn 配置与失败时禁用搜索](../crates/server/src/session_context.rs)
- [正常请求的 hosted gate 与 ipython surface](../crates/core/src/query/mod.rs)
- [hosted 参数结构](../crates/protocol/src/hosted_tools.rs) / [wire 序列化](../crates/provider/src/hosted_tools.rs)
- [hosted 事件消费](../crates/core/src/query/stream_consumer.rs)
- [RLM helpers](../crates/kernel/rlm-runtime/src/rlm/__init__.py) / [host bridge 与权限](../crates/server/src/runtime/kernel_host_bridge.rs)
- [内置搜索实现及测试](../crates/core/src/tools/handlers/websearch.rs) / [Parallel 实现](../crates/core/src/tools/handlers/websearch/parallel.rs)
- [本地抓取实现及测试](../crates/core/src/tools/handlers/webfetch.rs)
- [Serper 技能实现](../crates/skills/assets/bundled/websearch/src/websearch/websearch.py) / [登录入口](../apps/tui/lib/coding-agent/src/modes/interactive/auth-flows.ts)
- [MCP 配置结构](../crates/config/src/mcp.rs) / [HTTP header 解析](../crates/rmcp-client/src/utils.rs)
- [共享代理逻辑](../crates/network-proxy/src/lib.rs)

维护时至少重查：默认 mode、对象覆盖、认证迁移、hosted gate、上游工具版本、真实 wire 字段、RLM surface、第三方 endpoint/认证、技能 fallback 条件，以及“disabled”是否只是入口控制。实现和文档不一致时，应先记录差异，不能把计划中的能力描述为已实现。
