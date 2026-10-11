<div align="center">

<img src="./.github/assets/devo-readme-brand.svg" alt="Devo" width="360" />

</div>

# Devo — 开源递归智能体

Devo 通过**持久化 Python 工作区**、**子智能体**和**可自由选择的模型提供商**
处理多步骤任务。提供桌面应用和终端 TUI/CLI，由本地 Rust 运行时驱动。

[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)

[English](./README.md) · [简体中文](./README.zh-Hans.md) · [繁體中文](./README.zh-Hant.md) · [日本語](./README.ja.md) · [Русский](./README.ru.md)

## 与 Claude Code、Codex 的区别

| 智能体 | 核心侧重 | 模型接入 |
| --- | --- | --- |
| **Devo** | 通过可编程、递归执行完成编码及其他多步骤任务。 | 兼容的云端提供商、本地端点和私有网关。 |
| [Claude Code](https://code.claude.com/docs/en/overview) | 围绕 Claude 的开发工作流，覆盖终端、IDE、桌面和网页。 | 通过 Anthropic 或受支持的云平台接入 Claude。 |
| [Codex](https://learn.chatgpt.com/docs/codex/cli) | OpenAI 的编码智能体，覆盖终端、IDE、应用和云端。 | 默认使用 OpenAI 模型；CLI 也支持[自定义和本地提供商](https://learn.chatgpt.com/docs/config-file/config-advanced#custom-model-providers)。 |

Devo 的 `ipython` 工具会在调用之间保留变量和中间结果。智能体可以用 Python
筛选大量输出、复用已有工作，并将独立子任务交给拥有各自会话和 Python 状态的
子智能体。还支持 MCP、技能、权限控制和会话恢复。

## 快速开始

**桌面应用：** 从 [GitHub Releases](https://github.com/7df-lab/devo/releases/latest)
下载适合操作系统和架构的安装包，在首次设置中连接模型提供商。

**终端 — Linux / macOS：**

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

**终端 — Windows PowerShell：**

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

在工作目录启动；首次运行会打开提供商设置：

```bash
cd /path/to/your/workspace
devo
```

用 `devo resume <session-id>` 恢复会话，用 `devo upgrade` 升级。
终端安装包已包含 Node.js 和 Python。Windows 执行 shell 命令需要
[Git for Windows](https://gitforwindows.org/)，首次使用时还需批准一次沙箱设置提示。

## 配置

支持 **OpenAI 兼容 Chat Completions**、**OpenAI 兼容 Responses** 和
**Anthropic Messages** API。内置目录包含 DeepSeek、Qwen、Kimi、GLM 和 MiniMax。
用 `/model` 切换模型，用 `devo onboard` 重新配置提供商。

手动配置、凭据、MCP、离线安装和发布镜像见[使用指南（英文）](./docs/usage.md)。

## 文档与贡献

- [使用指南（英文）](./docs/usage.md) — 安装、提供商、配置与离线使用。
- [TUI 指南（英文）](./apps/tui/README.md) — 终端操作与命令。
- [贡献指南（英文）](./CONTRIBUTING.md) — 欢迎问题反馈、文档改进和小范围修复。

Devo 仍处于 **pre-1.0** 阶段，API 和配置可能变化。[MIT 许可证](./LICENSE)。
