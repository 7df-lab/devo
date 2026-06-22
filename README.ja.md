<div align="center">

<img src="./.github/assets/devo-readme-logo.png" alt="Devo" width="220" />

</div>

<div align="center">

**単一バイナリとして動作する、軽量でモデル中立なコーディングエージェント。高速で token 効率が高く、柔軟にカスタマイズできます。**

[![Stars](https://img.shields.io/github/stars/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/stargazers)
[![Language](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/7df-lab/devo/pulls)
[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)

[English](./README.md) | [简体中文](./README.zh-Hans.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [Русский](./README.ru.md)

[機能](#機能) · [検証済みモデル](#検証済みモデル) · [検証済みプラットフォーム](#検証済みプラットフォーム) · [インストール](#インストール) · [クイックスタート](#クイックスタート) · [ドキュメント](#docs)

</div>

---

## 機能

- **組み込みのセマンティックコード検索** - ローカル CPU のコード埋め込みモデルを実行し、
  dense retrieval と BM25 キーワードマッチングを組み合わせることで、grep/find のみに頼るエージェントより
  コード検索のコンテキストを削減します。
- **任意のモデルプロバイダーを利用可能** - provider/model binding により、OpenAI 互換、
  Anthropic 互換、DeepSeek、Qwen、Kimi、GLM、MiniMax、Xiaomi MiMo、
  OpenRouter、またはローカルエンドポイントを利用できます。
- **MCP サポート** - [Model Context Protocol](https://modelcontextprotocol.io/)
  サーバーを通じて外部ツールとコンテキストを接続できます。
- **Skill サポート** - 再利用可能なワークフロー、手順、スクリプト、参照資料を
  [Agent Skills](https://agentskills.io/) としてパッケージ化できます。
- **長時間タスクのサポート** - 複数ターンにまたがる作業でも Devo が自動的にコンテキストを管理し、
  タスクが大きくなっても流れを失いにくくします。
- **マルチエージェントサポート** - 専門エージェントに作業を分割しつつ、セッション内で調整状況を可視化します。
- **Plan Mode** - 実装を始める前に、大きなタスクを明確な複数ステップの計画へ分解します。
- **並列ツール呼び出し** - 独立した複数のツールを並列に実行し、モデルの待ち時間を減らして作業を進めます。
- **権限付きツール実行** - センシティブなツール呼び出しがワークスペースに触れる前にレビューできます。
- **監査可能なセッション** - モデル出力、ツール呼び出し、承認、token 使用量、セッション履歴を
  確認および再開できる形で保持します。
- **コストとコンテキストの可視化** - プロバイダーが提供する場合、入力/出力 token、cached token、
  コンテキストウィンドウ使用量を表示します。
- **軽量な Rust ランタイム** - Rust で構築され、メモリ使用量が小さく、コンパクトなローカルランタイムを備えます。

## 検証済みモデル

<p>
  <img alt="DeepSeek v4 Flash / Pro" src="https://img.shields.io/badge/DeepSeek-v4%20Flash%20%2F%20Pro-4D6BFE?style=flat-square&logo=deepseek&logoColor=white" />
  <img alt="Qwen3 Coder Next" src="https://img.shields.io/badge/Qwen3-Coder%20Next-615CED?style=flat-square&logo=qwen&logoColor=white" />
  <img alt="Kimi K2.5" src="https://img.shields.io/badge/Kimi-K2.5-111111?style=flat-square&logo=moonshotai&logoColor=white" />
  <img alt="MiniMax M2.7" src="https://img.shields.io/badge/MiniMax-M2.7-0B5FFF?style=flat-square&logo=minimax&logoColor=white" />
  <img alt="GLM 5.1" src="https://img.shields.io/badge/GLM-5.1-7856FF?style=flat-square" />
</p>

Devo の組み込みモデルカタログには、Qwen、Kimi、MiniMax、GLM、DeepSeek の検証済みモデル定義が含まれています。
プロバイダーのエンドポイントは provider/model binding で引き続き設定できます。

## 検証済みプラットフォーム

<p>
  <img alt="macOS 検証済み" src="https://img.shields.io/badge/macOS-tested-000000?style=flat-square&logo=apple&logoColor=white" />
  <img alt="Linux 検証済み" src="https://img.shields.io/badge/Linux-tested-FCC624?style=flat-square&logo=linux&logoColor=000000" />
  <img alt="Windows 検証済み" src="https://img.shields.io/badge/Windows-tested-0078D4?style=flat-square&logo=windows&logoColor=white" />
</p>

Devo は macOS、Linux、Windows、Kylin OS で検証済みです。

### 中国のエンタープライズユーザー向け

<p>
  <img alt="Kylin OS 検証済み" src="https://img.shields.io/badge/Kylin%20OS-tested-1E88E5?style=flat-square" />
  <img alt="HarmonyOS サポートはロードマップ上" src="https://img.shields.io/badge/HarmonyOS-on%20the%20road-111111?style=flat-square&logo=harmonyos&logoColor=white" />
</p>

中国のエンタープライズ環境では国産 OS が実際のデプロイ要件になることが多いため、Kylin OS の対応状況を明記しています。
HarmonyOS サポートはロードマップ上にあります。HarmonyOS デバイスを持つコントリビューターによる、そのプラットフォーム向けのビルド、テスト、リリースを歓迎します。

## スクリーンショット

<p align="center">
  <img width="100%" alt="ターミナルで実行中の Devo" src="./.github/assets/devo-readme-screenshot.png" />
</p>

## インストール

### Linux / macOS

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

### Windows

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

オンラインインストーラーは `devo` を Devo home ディレクトリに配置し、高速なリポジトリ検索に使う
`rg` sidecar をインストールします。また、`code_search` が使うローカル Hugging Face モデルを事前インストールできます。

ローカルの `code_search` モデルを事前インストールするには:

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --install-code-search-model
```

Windows:

```powershell
$env:DEVO_INSTALL_CODE_SEARCH_MODEL = "1"; irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

既存のインストールを最新 release にアップグレードするには:

```bash
devo upgrade
```

アップグレードコマンドは同じプラットフォーム用インストーラーを実行し、
インストーラーは `Version: v0.1.12 -> v0.1.15` のようにバージョン遷移を表示します。

<details>
<summary>オフラインインストール</summary>

多くのエンタープライズ環境やイントラネット環境ではインターネットにアクセスできません。
Devo のインストーラーはオフラインモードをサポートしており、必要なすべてのアセットをインストーラースクリプトと同じディレクトリから読み込み、ネットワークには接続しません。

インターネットにアクセスできるマシンで:

1. インストーラースクリプトをダウンロードします。Linux/macOS は `install.sh`、Windows は `install.ps1` です。
2. 対象 CPU と OS 向けの最新 Devo release asset をダウンロードします。例: `x86_64`
   と `aarch64`/`arm64`。
3. ローカルセマンティック `code_search` が使う Hugging Face `minishlab/potion-code-16M`
   モデルファイルをダウンロードします: `config.json`、`model.safetensors`、`tokenizer.json`。
4. 対象 CPU と OS に合う `ripgrep` release asset をダウンロードします。

これらのファイルをインストーラースクリプトの隣に置きます。モデルファイルはスクリプトの隣に直接置いても、
`minishlab--potion-code-16M/` サブディレクトリに置いても構いません。

Linux / macOS:

```bash
sh ./install.sh --offline
```

Windows:

```powershell
.\install.ps1 -Offline
```

オフラインモードでは、モデルは
`<DEVO_HOME>/local-models/minishlab--potion-code-16M` にインストールされます。
これはランタイムの code-search provider が使用するディレクトリです。
`DEVO_HOME` が設定されていない場合は
`~/.devo/local-models/minishlab--potion-code-16M` になります。

</details>

## クイックスタート

プロバイダーを設定し、リポジトリを開いて TUI を起動します:

```bash
cd /path/to/your/repo
devo onboard
```

便利なコマンド:

```bash
devo                         # 現在のリポジトリで対話型 TUI を起動
devo resume <session-id>
```

## 設定

`devo onboard` が推奨されるセットアップ方法です。手動で設定する場合、Devo は次の順序で設定をマージします:

1. 組み込みデフォルト
2. `DEVO_HOME/config.toml` - ユーザーレベル設定。デフォルトでは macOS/Linux で
   `~/.devo/config.toml`、Windows で `C:\Users\yourname\.devo\config.toml`
3. `<workspace>/.devo/config.toml` - プロジェクトレベル設定
4. CLI flags

認証情報は `DEVO_HOME/auth.json` に分離して保存されます。
`config.toml` には API key を直接保存せず、credential id を参照させてください。

最小構成:

```toml
[defaults]
model_binding = "deepseek-v4-flash-api-deepseek-com"

[providers."api.deepseek.com"]
enabled = true
name = "api.deepseek.com"
base_url = "https://api.deepseek.com"
credential = "api_deepseek_com_api_key"
wire_apis = ["openai_chat_completions"]

[model_bindings.deepseek-v4-flash-api-deepseek-com]
enabled = true
model_slug = "deepseek-v4-flash"
provider = "api.deepseek.com"
model_name = "deepseek-v4-flash"
display_name = "DeepSeek V4 Flash"
invocation_method = "openai_chat_completions"
default_reasoning_effort = "high"
```

重要な分離は次のとおりです:

- `model_slug` は `models.json` から Devo のローカルモデルメタデータを選択します。
- `provider` は設定済みの接続レコードを選択します。
- `model_name` はプロバイダーへ送信される、そのプロバイダー固有のモデル文字列です。
- `invocation_method` はプロバイダープロトコルを選択します。例:
  [`openai_chat_completions`](https://developers.openai.com/api/reference/chat-completions/overview)、
  [`openai_responses`](https://developers.openai.com/api/reference/responses/overview)、
  [`anthropic_messages`](https://platform.claude.com/docs/en/api/messages)。

### カスタムモデル

使いたいモデルが組み込みリストにない場合は、`models.json` に追加してから
`config.toml` でバインドします。

ユーザーレベルのモデルカタログ:

- macOS/Linux: `~/.devo/models.json`
- Windows: `C:\Users\yourname\.devo\models.json`

プロジェクトレベルの上書きは `<workspace>/.devo/models.json` に配置できます。
`models.json` の `provider` は、そのモデルのデフォルト wire API メタデータです。
実際のエンドポイントは引き続き `config.toml` の `provider` フィールドで選択されます。

`models.json` エントリの例:

```json
[
  {
    "slug": "my-coding-model",
    "display_name": "My Coding Model",
    "channel": "Custom",
    "provider": "openai_chat_completions",
    "description": "Custom OpenAI-compatible coding model.",
    "reasoning_capability": "unsupported",
    "context_window": 200000,
    "effective_context_window_percent": 95,
    "max_tokens": 4096,
    "input_modalities": ["text"],
    "base_instructions": "You are Devo, a coding agent. Help the user edit and understand code."
  }
]
```

次に、その `slug` を model binding から参照します:

```toml
[model_bindings.my-coding-model-example]
enabled = true
model_slug = "my-coding-model"
provider = "my.provider"
model_name = "provider-specific-model-name"
display_name = "My Coding Model"
invocation_method = "openai_chat_completions"
```

## よくある質問

### プロジェクトの状態は?

Devo は pre-1.0 で、活発に開発されています。ローカル評価、実験、コントリビューターによる利用には適していますが、
公開 API と設定は今後も変更される可能性があります。

### どのモデルがサポートされていますか?

組み込みモデルメタデータは現在、Qwen、Kimi、MiniMax、GLM、DeepSeek ファミリーをカバーしています。
OpenAI 互換 Chat Completions、OpenAI 互換 Responses、または Anthropic Messages API をサポートするモデルエンドポイントであれば、
provider/model binding を通じて接続できます。

## コントリビュート

プロジェクトはまだ初期段階であり、コントリビューションを歓迎します:

- client/server runtime、provider layer、safety model、TUI に関するアーキテクチャフィードバック。
- ドキュメントと翻訳。
- Provider、model、wire API の対応範囲。
- 検証コマンドと回帰テストを伴う、焦点を絞った修正。

変更について議論するには issue または pull request を開いてください。

## Star 履歴

<a href="https://www.star-history.com/?repos=7df-lab%2Fdevo&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&legend=top-left" />
 </picture>
</a>

## ライセンス

このプロジェクトは [MIT License](./LICENSE) のもとで公開されています。

---

**Devo が役に立った場合は、star をご検討ください。**
