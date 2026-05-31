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

**Открытый агент для программирования, который работает очень быстро, безопасен и не зависит от конкретного поставщика моделей.**

🚧Проект на ранней стадии активной разработки — пока не готов к production.
⭐ Поставьте звезду, чтобы следить за проектом

[![Stars](https://img.shields.io/github/stars/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/stargazers)
[![Language](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/7df-lab/devo/pulls)
[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)

[English](./README.md) | [简体中文](./README.zh-Hans.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [Русский](./README.ru.md)

</div>

---

## 📖 Содержание

- [Быстрый старт](#-быстрый-старт)
- [Установка](#-установка)
- [Часто задаваемые вопросы](#-часто-задаваемые-вопросы)
- [Участие в разработке](#-участие-в-разработке)
- [Ссылки](#-ссылки)
- [Лицензия](#-лицензия)

## 📦 Установка

### Linux / macOS

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

### Windows

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

## 🚀 Быстрый старт

Если вы предпочитаете собрать из исходников, используйте инструкции ниже.

### Сборка

```bash
git clone https://github.com/7df-lab/devo && cd devo
cargo build --release

# linux / macos
./target/release/devo onboard

# windows
.\target\release\devo onboard
```

> [!TIP]
> Убедитесь, что Rust установлен; рекомендуется версия 1.75+ (через https://rustup.rs/).

## ⚙️ Конфигурация

Devo читает конфигурацию из TOML-файла, объединяя источники с более высоким приоритетом поверх низкоприоритетных:

1. Встроенные значения по умолчанию (скомпилированы в бинарный файл)
2. `DEVO_HOME/config.toml` — пользовательская конфигурация (по умолчанию `~/.devo/config.toml` на macOS/linux, `C:\Users\yourname\.devo\config.toml` на Windows)
3. `<workspace>/.devo/config.toml` — конфигурация уровня проекта
4. CLI флаги — переопределения из командной строки

Оба конфигурационных файла необязательны. Минимальный файл конфигурации должен содержать только секцию провайдера, чтобы devo знал, какую модель использовать. Запустите `devo onboard` для интерактивной настройки.

### Пример минимальной конфигурации

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

## Часто задаваемые вопросы

### Чем это отличается от Claude Code?

По возможностям проект очень похож на Claude Code. Основные отличия:

- 100% open source
- Не привязан к одному провайдеру. Devo можно использовать с Claude, OpenAI, z.ai, Qwen, Deepseek или даже с локальными моделями. По мере развития моделей разрыв между ними будет сокращаться, а стоимость снижаться, поэтому независимость от провайдера важна.
- TUI уже реализован
- Построен на клиент-серверной архитектуре. Например, ядро может работать локально на вашем компьютере и при этом управляться удалённо, например из мобильного приложения, а TUI будет лишь одним из возможных клиентов

## 🤝 Участие в разработке

Мы приветствуем вклад в проект. Он находится на ранней стадии проектирования, и помочь можно разными способами:

- **Обратная связь по архитектуре** — изучите дизайн крейтов и предложите улучшения
- **Обсуждение RFC** — предлагайте новые идеи через issues
- **Документация** — помогайте улучшать или переводить документацию
- **Реализация** — подключайтесь к реализации крейтов, когда дизайн стабилизируется

Не стесняйтесь открывать issue или отправлять pull request.

## 📄 Лицензия

Проект распространяется по [лицензии MIT](./LICENSE).

---

**Если проект оказался полезным, поставьте ему ⭐**
