<div align="center">

# 🦀 Claw CR

**Rust で構築された、オープンソースのコーディングエージェント。Claude Code の代替です。**

[![ステータス](https://img.shields.io/badge/status-designing-blue?style=flat-square)](https://github.com/)
[![言語](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![由来](https://img.shields.io/badge/origin-Claude_Code_TS-8A2BE2?style=flat-square)](https://docs.anthropic.com/en/docs/claude-code)
[![ライセンス](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/)

[English](./README.md) | [简体中文](./README.zh-CN.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Español](./README.es.md) | [Français](./README.fr.md)

🚧 初期段階のプロジェクトで、活発に開発中です。まだ本番利用向けではありません。

⭐ フォローのためにスターをお願いします

<img
  src="./docs/assets/screenshot_20260408.png"
  alt="Resumen del proyecto"
  width="100%"
  style="border-radius: 8px; box-shadow: 0 15px 40px rgba(0,0,0,0.25);object-fit:cover;"
/>


</div>

---

## 📖 目次

- [クイックスタート](#-クイックスタート)
- [設計目標](#-設計目標)
- [コントリビュート](#-コントリビュート)
- [参考資料](#-参考資料)
- [ライセンス](#-ライセンス)

## 🚀 クイックスタート

まだ安定版はありません。以下の手順でソースからビルドできます。

### ビルド

```bash
git clone https://github.com/claw-cli/claw-code-rust && cd claw-code-rust
cargo build --release

# linux / macos
./target/release/clawcr onboard

# windows
.\target\release\clawcr onboard
```

> [!TIP]
> Rust がインストールされていることを確認してください。1.75+ を推奨します（https://rustup.rs/）。

## FAQ

### Claude Code と何が違いますか？

機能面では Claude Code と非常に近いです。主な違いは次のとおりです。

- 100% オープンソース
- 特定のプロバイダーに依存しません。ClawCR は Claude、OpenAI、z.ai、Qwen、Deepseek、あるいはローカルモデルでも利用できます。モデルが進化するにつれて差は縮まり、価格も下がっていくため、プロバイダー非依存であることは重要です。
- LSP を最初からサポート
- TUI のサポートはすでに実装済み
- クライアント/サーバー型アーキテクチャで構築されています。たとえば、core はローカルマシンで動かしつつ、モバイルアプリなどからリモート制御でき、TUI は複数あるクライアントの1つにすぎません。

## 🤝 コントリビュート

コントリビュートを歓迎します。このプロジェクトはまだ設計初期段階で、協力できることはたくさんあります。

- **アーキテクチャのフィードバック** — crate 設計をレビューして改善案を提案する
- **RFC ディスカッション** — issue を通じて新しいアイデアを提案する
- **ドキュメント** — ドキュメントの改善や翻訳を手伝う
- **実装** — 設計が安定したら実装 crate を担当する

issue を開くか pull request を送ってください。

## 📄 ライセンス

このプロジェクトは [MIT ライセンス](./LICENSE) の下で公開されています。

---

**このプロジェクトが役に立ったら、⭐ をお願いします**
