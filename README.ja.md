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

**超高速で安全、モデルプロバイダーに依存しないオープンソースのコーディングエージェント。**

🚧早期段階のプロジェクトで活発に開発中 — まだ本番環境の準備はできていません。
⭐ スターをつけてフォローしてください

[![Stars](https://img.shields.io/github/stars/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/stargazers)
[![Language](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/7df-lab/devo/pulls)
[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)

[English](./README.md) | [简体中文](./README.zh-Hans.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [Русский](./README.ru.md)

</div>

---

## 📖 目次

- [クイックスタート](#-クイックスタート)
- [インストール](#-インストール)
- [よくある質問](#-よくある質問)
- [コントリビュート](#-コントリビュート)
- [参考](#-参考)
- [ライセンス](#-ライセンス)

## 📦 インストール

### Linux / macOS

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

### Windows

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

## 🚀 クイックスタート

ソースからビルドしたい場合は、以下の手順を使用してください。

### ビルド

```bash
git clone https://github.com/7df-lab/devo && cd devo
cargo build --release

# linux / macos
./target/release/devo onboard

# windows
.\target\release\devo onboard
```

> [!TIP]
> Rust がインストールされていることを確認してください。1.75+ を推奨します（https://rustup.rs/ から）。

## ⚙️ 設定

Devo は TOML ファイルから設定を読み込み、優先度の高いソースが低いものを上書きします：

1. ビルトインデフォルト（バイナリにコンパイル）
2. `DEVO_HOME/config.toml` — ユーザーレベル設定（デフォルト: macOS/linux では `~/.devo/config.toml`、Windows では `C:\Users\yourname\.devo\config.toml`）
3. `<workspace>/.devo/config.toml` — プロジェクトレベル設定
4. CLI フラグ — コマンドラインでの上書き

両方の設定ファイルはオプションです。最小限の設定ファイルには、devo がどのモデルを使用するかを知らせる provider セクションのみが必要です。`devo onboard` を実行すると、インタラクティブなセットアップでこれが書き込まれます。

### 最小設定例

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

## よくある質問

### これは Claude Code と何が違いますか？

機能面では Claude Code と非常に似ています。主な違いは次のとおりです。

- 100% オープンソース
- 特定のプロバイダーに依存しません。Devo は Claude、OpenAI、z.ai、Qwen、Deepseek、あるいはローカルモデルでも利用できます。モデルが進化するにつれて差は縮まり、価格も下がっていくため、プロバイダー非依存であることは重要です。
- TUI サポートはすでに実装済みです
- クライアント/サーバー型アーキテクチャで構築されています。たとえば、コアはローカルマシンで動作しつつ、モバイルアプリなどからリモート制御でき、TUI は複数あるクライアントの1つにすぎません。

## 🤝 コントリビュート

コントリビュートを歓迎します。このプロジェクトはまだ設計初期段階で、協力できる方法がたくさんあります。

- **アーキテクチャのフィードバック** — crate 設計をレビューして改善案を提案する
- **RFC ディスカッション** — issue を通じて新しいアイデアを提案する
- **ドキュメント** — ドキュメントの改善や翻訳を手伝う
- **実装** — 設計が安定したら実装 crate を担当する

issue を開くか pull request を送ってください。

## 📄 ライセンス

このプロジェクトは [MIT ライセンス](./LICENSE) のもとで公開されています。

---

**このプロジェクトが役に立ったら、⭐ をお願いします**
