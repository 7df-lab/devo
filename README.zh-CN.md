<div align="center">

# 🦀 Claw CR

**用 Rust 构建的开源编程代理。Claude Code 的替代方案。**

[![状态](https://img.shields.io/badge/status-designing-blue?style=flat-square)](https://github.com/)
[![语言](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![来源](https://img.shields.io/badge/origin-Claude_Code_TS-8A2BE2?style=flat-square)](https://docs.anthropic.com/en/docs/claude-code)
[![许可](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![欢迎 PR](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/)

[English](./README.md) | [简体中文](./README.zh-CN.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Español](./README.es.md) | [Français](./README.fr.md)

🚧 这是一个早期阶段且正在积极开发中的项目，还不适合生产使用。

⭐ 欢迎点个星关注

<img
  src="./docs/assets/screenshot_20260408.png"
  alt="Resumen del proyecto"
  width="100%"
  style="border-radius: 8px; box-shadow: 0 15px 40px rgba(0,0,0,0.25);object-fit:cover;"
/>

</div>

---

## 📖 目录

- [快速开始](#-快速开始)
- [设计目标](#-设计目标)
- [参与贡献](#-参与贡献)
- [参考资料](#-参考资料)
- [许可证](#-许可证)

## 🚀 快速开始

目前还没有稳定版本，你可以按照下面的说明从源码构建项目。

### 构建

```bash
git clone https://github.com/claw-cli/claw-code-rust && cd claw-code-rust
cargo build --release

# linux / macos
./target/release/clawcr onboard

# windows
.\target\release\clawcr onboard
```

> [!TIP]
> 请确保已安装 Rust，推荐 1.75+（通过 https://rustup.rs/ 安装）。

## FAQ

### 这和 Claude Code 有什么不同？

在能力上，它和 Claude Code 非常接近。主要区别如下：

- 100% 开源
- 不绑定任何特定提供方。ClawCR 可以配合 Claude、OpenAI、z.ai、Qwen、Deepseek，甚至本地模型使用。随着模型不断演进，差距会缩小，价格也会下降，因此保持 provider 无关性很重要。
- 开箱即用的 LSP 支持
- TUI 支持已经实现
- 采用 client/server 架构。例如，core 可以在本机运行，同时由远程控制，比如从移动端应用控制，而 TUI 只是众多 client 之一。

## 🤝 参与贡献

欢迎贡献！这个项目还处在早期设计阶段，有很多方式可以参与：

- **架构反馈** — 审阅 crate 设计并提出改进建议
- **RFC 讨论** — 通过 issue 提出新想法
- **文档** — 帮助改进或翻译文档
- **实现** — 设计稳定后参与实现 crate

欢迎随时提 issue 或提交 pull request。

## 📄 许可证

本项目采用 [MIT 许可证](./LICENSE)。

---

**如果这个项目对你有帮助，欢迎点个 ⭐**
