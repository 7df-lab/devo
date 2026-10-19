<div align="center">

<img src="./.github/assets/devo-readme-brand.svg" alt="Devo" width="360" />

</div>

# Devo — 開源遞迴智慧代理

Devo 透過**持久化 Python 工作區**、**子代理**和**可自由選擇的模型供應商**
處理多步驟任務。提供桌面應用程式和終端 TUI/CLI，由本機 Rust 執行環境驅動。

[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)

[English](./README.md) · [简体中文](./README.zh-Hans.md) · [繁體中文](./README.zh-Hant.md) · [日本語](./README.ja.md) · [Русский](./README.ru.md)

## 與 Claude Code、Codex 的差異

| 代理 | 核心側重 | 模型接入 |
| --- | --- | --- |
| **Devo** | 透過可程式化、遞迴執行完成程式開發及其他多步驟任務。 | 相容的雲端供應商、本機端點和私有閘道。 |
| [Claude Code](https://code.claude.com/docs/en/overview) | 圍繞 Claude 的開發工作流程，涵蓋終端、IDE、桌面和網頁。 | 透過 Anthropic 或支援的雲端平台接入 Claude。 |
| [Codex](https://learn.chatgpt.com/docs/codex/cli) | OpenAI 的程式開發代理，涵蓋終端、IDE、應用程式和雲端。 | 預設使用 OpenAI 模型；CLI 也支援[自訂和本機供應商](https://learn.chatgpt.com/docs/config-file/config-advanced#custom-model-providers)。 |

Devo 的 `ipython` 工具會在呼叫之間保留變數和中間結果。代理可以用 Python
篩選大量輸出、重用既有工作，並將獨立子任務交給擁有各自工作階段和 Python 狀態的
子代理。也支援 MCP、技能、權限控制和工作階段恢復。

## 快速開始

**桌面應用程式：** 從 [GitHub Releases](https://github.com/7df-lab/devo/releases/latest)
下載適合作業系統和架構的安裝套件，在首次設定中連接模型供應商。

**終端 — Linux / macOS：**

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

**終端 — Windows PowerShell：**

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

在工作目錄啟動；首次執行會開啟供應商設定：

```bash
cd /path/to/your/workspace
devo
```

用 `devo resume <session-id>` 恢復工作階段，用 `devo upgrade` 升級。
終端安裝套件已包含 Node.js 和 Python。Windows 執行 shell 命令需要
[Git for Windows](https://gitforwindows.org/)，首次使用時也需核准一次沙箱設定提示。

## 設定

支援 **OpenAI 相容 Chat Completions**、**OpenAI 相容 Responses** 和
**Anthropic Messages** API。內建目錄包含 DeepSeek、Qwen、Kimi、GLM 和 MiniMax。
用 `/model` 切換模型，用 `devo onboard` 重新設定供應商。

手動設定、憑證、MCP、離線安裝和發行鏡像請見[使用指南（英文）](./docs/usage.md)。

## 文件與貢獻

- [使用指南（英文）](./docs/usage.md) — 安裝、供應商、設定與離線使用。
- [TUI 指南（英文）](./apps/tui/README.md) — 終端操作與命令。
- [貢獻指南（英文）](./CONTRIBUTING.md) — 歡迎問題回報、文件改進和小範圍修正。

Devo 仍處於 **pre-1.0** 階段，API 和設定可能變動。[MIT 授權條款](./LICENSE)。
