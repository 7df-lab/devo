<div align="center">

<pre style="font-size: 14px; line-height: 1.15; display: inline-block; text-align: left; color: #60A5FA;">
██████╗  ███████╗██╗   ██╗ ██████╗
██╔══██╗ ██╔════╝██║   ██║██╔═══██╗
██║  ██║ █████╗  ██║   ██║██║   ██║
██║  ██║ ██╔══╝  ╚██╗ ██╔╝██║   ██║
██████╔╝ ███████╗ ╚████╔╝ ╚██████╔╝
╚═════╝  ╚══════╝  ╚═══╝   ╚═════╝
</pre>

</div>

<div align="center">

**一個開源程式碼代理，極其快速、安全且與模型提供商無關。**

🚧早期專案正在積極開發中 — 尚未準備好投入生產。
⭐ 點星關注我們

[![Stars](https://img.shields.io/github/stars/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/stargazers)
[![Language](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/7df-lab/devo/pulls)
[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)

[English](./README.md) | [简体中文](./README.zh-Hans.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [Русский](./README.ru.md)

</div>

---

## 📖 目錄

- [快速開始](#-快速開始)
- [安裝](#-安裝)
- [常見問題](#-常見問題)
- [參與貢獻](#-參與貢獻)
- [參考](#-參考)
- [授權](#-授權)

## 📦 安裝

### Linux / macOS

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

### Windows

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

## 🚀 快速開始

如果你更想從原始碼建置，可以使用下面的說明。

### 建置

```bash
git clone https://github.com/7df-lab/devo && cd devo
cargo build --release

# linux / macos
./target/release/devo onboard

# windows
.\target\release\devo onboard
```

> [!TIP]
> 確保已安裝 Rust，推薦 1.75+（透過 https://rustup.rs/ 安裝）。

## ⚙️ 配置

Devo 從 TOML 檔案讀取配置，高優先級來源覆蓋低優先級來源：

1. 內建預設值（編譯在二進位檔中）
2. `DEVO_HOME/config.toml` — 使用者級配置（預設：macOS/linux 為 `~/.devo/config.toml`，Windows 為 `C:\Users\yourname\.devo\config.toml`）
3. `<workspace>/.devo/config.toml` — 專案級配置
4. CLI 標誌 — 命令列覆蓋

兩個配置檔案都是可選的。最小配置檔案只需要一個 provider 部分，讓 devo 知道使用哪個模型。執行 `devo onboard` 進行互動式設定。

### 最小配置範例

```toml
# ~/.devo/config.toml
model = "deepseek-v4-flash"
model_provider = "api.deepseek.com"
model_thinking_selection = "high"

[model_providers."api.deepseek.com"]
name = "api.deepseek.com"
api_key = "sk-..."
base_url = "https://api.deepseek.com"
wire_api = "openai_chat_completions"

[[model_providers."api.deepseek.com".models]]
model = "deepseek-v4-pro"

[[model_providers."api.deepseek.com".models]]
model = "deepseek-v4-flash"
```

## Star us

<a href="https://www.star-history.com/?repos=7df-lab%2Fdevo&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&legend=top-left" />
 </picture>
</a>

## 常見問題

### 這和 Claude Code 有什麼不同？

在能力上，它和 Claude Code 非常相似。主要差異如下：

- 100% 開源
- 不綁定任何供應商。Devo 可以搭配 Claude、OpenAI、z.ai、Qwen、Deepseek，甚至本地模型使用。隨著模型持續演進，差距會縮小，價格也會下降，因此保持 provider 無關非常重要。
- TUI 支援已實現
- 採用客戶端/伺服器架構。例如，核心可以在你的本機執行，同時由遠端控制（比如透過手機 App 操作），而 TUI 只是眾多客戶端之一。

## 🤝 參與貢獻

歡迎貢獻！這個專案仍處於早期設計階段，有很多方式可以參與：

- **架構回饋** — 審查 crate 設計並提出改善建議
- **RFC 討論** — 透過 issue 提出新想法
- **文件** — 協助改善或翻譯文件
- **實作** — 等設計穩定後，協助推進 crate 實作

歡迎直接開 issue 或提交 pull request。

## 📄 授權

本專案採用 [MIT 授權](./LICENSE)。

---

**如果你覺得這個專案有幫助，歡迎給它一個 ⭐**
