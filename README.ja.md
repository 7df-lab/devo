<div align="center">

<img src="./.github/assets/devo-readme-brand.svg" alt="Devo" width="360" />

</div>

# Devo — オープンソースの再帰型エージェント

Devo は**状態を保持する Python ワークスペース**、**子エージェント**、
**自由に選べるモデルプロバイダー**で複数ステップのタスクを進めます。
ローカルの Rust ランタイムを、デスクトップアプリまたはターミナル TUI/CLI から利用できます。

[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)

[English](./README.md) · [简体中文](./README.zh-Hans.md) · [繁體中文](./README.zh-Hant.md) · [日本語](./README.ja.md) · [Русский](./README.ru.md)

## Claude Code・Codex との違い

| エージェント | 主な特徴 | モデル接続 |
| --- | --- | --- |
| **Devo** | プログラムによる再帰的な実行で、開発やその他の複数ステップのタスクを処理。 | 互換クラウドプロバイダー、ローカルエンドポイント、プライベートゲートウェイ。 |
| [Claude Code](https://code.claude.com/docs/en/overview) | Claude を中心とした開発ワークフロー。ターミナル、IDE、デスクトップ、Web に対応。 | Anthropic または対応クラウドプロバイダー経由の Claude。 |
| [Codex](https://learn.chatgpt.com/docs/codex/cli) | OpenAI の開発エージェント。ターミナル、IDE、アプリ、クラウドに対応。 | 標準は OpenAI モデル。CLI は[カスタム・ローカルプロバイダー](https://learn.chatgpt.com/docs/config-file/config-advanced#custom-model-providers)にも対応。 |

Devo の `ipython` ツールは、呼び出し間で変数や中間結果を保持します。
Python で大量の出力を絞り込み、先行作業を再利用し、独立したサブタスクを
専用のセッションと Python 状態を持つ子エージェントに委任できます。
MCP、スキル、権限制御、セッション再開にも対応しています。

## クイックスタート

**デスクトップ：** [GitHub Releases](https://github.com/7df-lab/devo/releases/latest)
から OS とアーキテクチャに合うパッケージをダウンロードし、初期設定でモデルプロバイダーを接続します。

**ターミナル — Linux / macOS：**

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

**ターミナル — Windows PowerShell：**

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

作業ディレクトリで起動します。初回はプロバイダー設定が開きます。

```bash
cd /path/to/your/workspace
devo
```

`devo resume <session-id>` で再開、`devo upgrade` で更新できます。
ターミナルのパッケージには Node.js と Python が含まれています。
Windows のシェルコマンドには [Git for Windows](https://gitforwindows.org/) が必要です。
初回利用時にはサンドボックス設定の確認も一度承認してください。

## 設定

**OpenAI 互換 Chat Completions**、**OpenAI 互換 Responses**、
**Anthropic Messages** API に対応しています。組み込みカタログには
DeepSeek、Qwen、Kimi、GLM、MiniMax が含まれます。
`/model` でモデルを切り替え、`devo onboard` でプロバイダーを再設定できます。

手動設定、認証情報、MCP、オフラインインストール、リリースミラーは
[ユーザーガイド（英語）](./docs/usage.md)を参照してください。

## ドキュメントと貢献

- [ユーザーガイド（英語）](./docs/usage.md) — インストール、プロバイダー、設定、オフライン利用。
- [TUI ガイド（英語）](./apps/tui/README.md) — ターミナル操作とコマンド。
- [貢献ガイド（英語）](./CONTRIBUTING.md) — 問題報告、ドキュメント改善、小規模な修正を歓迎します。

Devo は **pre-1.0** です。API や設定は変更される場合があります。[MIT ライセンス](./LICENSE)。
