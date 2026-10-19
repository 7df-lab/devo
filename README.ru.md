<div align="center">

<img src="./.github/assets/devo-readme-brand.svg" alt="Devo" width="360" />

</div>

# Devo — рекурсивный агент с открытым исходным кодом

Devo решает многошаговые задачи с помощью **Python-среды с сохранением состояния**,
**дочерних агентов** и **модельного провайдера на ваш выбор**.
Десктопное приложение и терминальный TUI/CLI работают на локальном Rust runtime.

[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)

[English](./README.md) · [简体中文](./README.zh-Hans.md) · [繁體中文](./README.zh-Hant.md) · [日本語](./README.ja.md) · [Русский](./README.ru.md)

## Отличия от Claude Code и Codex

| Агент | Основной акцент | Модели |
| --- | --- | --- |
| **Devo** | Программируемое рекурсивное выполнение задач разработки и других многошаговых задач. | Совместимые облачные провайдеры, локальные эндпоинты и приватные шлюзы. |
| [Claude Code](https://code.claude.com/docs/en/overview) | Разработка с Claude в терминале, IDE, десктопном приложении и браузере. | Claude через Anthropic или поддерживаемых облачных провайдеров. |
| [Codex](https://learn.chatgpt.com/docs/codex/cli) | Агент OpenAI для разработки в терминале, IDE, приложении и облаке. | По умолчанию модели OpenAI; CLI также поддерживает [собственных и локальных провайдеров](https://learn.chatgpt.com/docs/config-file/config-advanced#custom-model-providers). |

Инструмент `ipython` сохраняет переменные и промежуточные результаты между вызовами.
Агент может фильтровать большие объёмы вывода в Python, повторно использовать
результаты и делегировать независимые подзадачи дочерним агентам с отдельными
сессиями и состоянием Python. Также доступны MCP, навыки, контроль разрешений
и возобновление сессий.

## Быстрый старт

**Десктоп:** Скачайте пакет для вашей ОС и архитектуры из
[GitHub Releases](https://github.com/7df-lab/devo/releases/latest) и подключите
модельного провайдера при первоначальной настройке.

**Терминал — Linux / macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

**Терминал — Windows PowerShell:**

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

Запустите Devo в рабочей директории; при первом запуске откроется настройка провайдера:

```bash
cd /path/to/your/workspace
devo
```

Возобновление: `devo resume <session-id>`; обновление: `devo upgrade`.
Терминальные пакеты включают Node.js и Python. В Windows для команд оболочки
нужен [Git for Windows](https://gitforwindows.org/); при первом использовании
также подтвердите однократную настройку песочницы.

## Конфигурация

Поддерживаются API **OpenAI-совместимый Chat Completions**,
**OpenAI-совместимый Responses** и **Anthropic Messages**. Встроенный каталог
включает DeepSeek, Qwen, Kimi, GLM и MiniMax.
Переключайте модели через `/model`, повторно настраивайте провайдера через `devo onboard`.

Ручная конфигурация, учётные данные, MCP, офлайн-установка и зеркала релизов
описаны в [руководстве пользователя (на английском)](./docs/usage.md).

## Документация и участие

- [Руководство пользователя (на английском)](./docs/usage.md) — установка, провайдеры, конфигурация и офлайн-работа.
- [Руководство TUI (на английском)](./apps/tui/README.md) — управление терминалом и команды.
- [Участие в разработке (на английском)](./CONTRIBUTING.md) — приветствуются сообщения об ошибках, улучшения документации и небольшие исправления.

Devo находится на стадии **pre-1.0**; API и конфигурация могут меняться. [Лицензия MIT](./LICENSE).
