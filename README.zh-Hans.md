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

**一个开源编程代理，极其快速、安全且与模型提供商无关。**

🚧早期项目正在积极开发中 — 尚未准备好投入生产。
⭐ 点星关注我们

[![Stars](https://img.shields.io/github/stars/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/stargazers)
[![Language](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/7df-lab/devo/pulls)
[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)

[English](./README.md) | [简体中文](./README.zh-Hans.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [Русский](./README.ru.md)

</div>

---

## 📖 目录

- [安装](#-安装)
- [快速开始](#-快速开始)
- [常见问题](#-常见问题)
- [参与贡献](#-参与贡献)
- [参考](#-参考)
- [许可证](#-许可证)

## 📦 安装

### Linux / macOS

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

### Windows

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

## 🚀 快速开始

如果你更想从源码构建，可以使用下面的说明。

### 构建

```bash
git clone https://github.com/7df-lab/devo && cd devo
cargo build --release

# linux / macos
./target/release/devo onboard

# windows
.\target\release\devo onboard
```

> [!TIP]
> 确保已安装 Rust，推荐 1.75+（通过 https://rustup.rs/ 安装）。

## ⚙️ 配置

Devo 从 TOML 文件读取配置，高优先级源覆盖低优先级源：

1. 内置默认值（编译在二进制中）
2. `DEVO_HOME/config.toml` — 用户级配置（默认：macOS/linux 为 `~/.devo/config.toml`，Windows 为 `C:\Users\yourname\.devo\config.toml`）
3. `<workspace>/.devo/config.toml` — 项目级配置
4. CLI 标志 — 命令行覆盖

两个配置文件都是可选的。最小配置文件只需要一个 provider 部分，让 devo 知道使用哪个模型。运行 `devo onboard` 进行交互式设置。

### 最小配置示例

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

## 常见问题

### 这和 Claude Code 有什么不同？

在能力上，它和 Claude Code 非常相似。主要区别如下：

- 100% 开源
- 不绑定任何提供商。Devo 可以配合 Claude、OpenAI、z.ai、Qwen、Deepseek，甚至本地模型使用。随着模型不断演进，差距会缩小，价格也会下降，因此保持 provider 无关性很重要。
- TUI 支持已实现
- 采用客户端/服务器架构。例如，核心可以在本机运行，同时由远程控制（比如从移动应用控制），而 TUI 只是众多客户端之一。

## 🤝 参与贡献

欢迎贡献！这个项目还处于早期设计阶段，有很多方式可以参与：

- **架构反馈** — 审查 crate 设计并提出改进建议
- **RFC 讨论** — 通过 issue 提出新想法
- **文档** — 帮助改进或翻译文档
- **实现** — 设计稳定后参与实现 crate

欢迎随时提 issue 或提交 pull request。

## 📄 许可

本项目采用 [MIT 许可证](./LICENSE)。

---

**如果这个项目对你有帮助，欢迎点个 ⭐**
