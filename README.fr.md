<div align="center">

# 🦀 Claw CR

**L’agent de codage open source, écrit en Rust. Une alternative à Claude Code.**

[![Statut](https://img.shields.io/badge/status-designing-blue?style=flat-square)](https://github.com/)
[![Langue](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Origine](https://img.shields.io/badge/origin-Claude_Code_TS-8A2BE2?style=flat-square)](https://docs.anthropic.com/en/docs/claude-code)
[![Licence](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/)

[English](./README.md) | [简体中文](./README.zh-CN.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Español](./README.es.md) | [Français](./README.fr.md)

🚧 Projet en phase initiale et en développement actif. Pas encore prêt pour la production.

⭐ Ajoutez une étoile pour suivre le projet

<img
  src="./docs/assets/screenshot_20260408.png"
  alt="Resumen del proyecto"
  width="100%"
  style="border-radius: 8px; box-shadow: 0 15px 40px rgba(0,0,0,0.25);object-fit:cover;"
/>

</div>

---

## 📖 Table des matières

- [Démarrage rapide](#-démarrage-rapide)
- [Objectifs de conception](#-objectifs-de-conception)
- [Contribuer](#-contribuer)
- [Références](#-références)
- [Licence](#-licence)

## 🚀 Démarrage rapide

Il n’existe pas encore de version stable. Vous pouvez compiler le projet depuis les sources avec les instructions ci-dessous.

### Compilation

```bash
git clone https://github.com/claw-cli/claw-code-rust && cd claw-code-rust
cargo build --release

# linux / macos
./target/release/clawcr onboard

# windows
.\target\release\clawcr onboard
```

> [!TIP]
> Assurez-vous d’avoir Rust installé ; la version 1.75+ est recommandée (via https://rustup.rs/).

## FAQ

### En quoi est-ce différent de Claude Code ?

C’est très similaire à Claude Code en termes de capacités. Voici les principales différences :

- 100 % open source
- Pas lié à un fournisseur. ClawCR peut fonctionner avec Claude, OpenAI, z.ai, Qwen, Deepseek ou même des modèles locaux. À mesure que les modèles évoluent, l’écart se réduira et les prix baisseront, donc l’indépendance vis-à-vis du fournisseur est importante.
- Prise en charge LSP prête à l’emploi
- La prise en charge TUI est déjà implémentée
- Construit avec une architecture client/serveur. Par exemple, le core peut s’exécuter localement sur votre machine tout en étant contrôlé à distance, par exemple depuis une application mobile, la TUI n’étant qu’un client parmi d’autres.

## 🤝 Contribuer

Les contributions sont les bienvenues ! Ce projet est encore dans une phase de conception précoce, et il existe de nombreuses façons d’aider :

- **Retours d’architecture** — Relire la conception des crates et proposer des améliorations
- **Discussions RFC** — Proposer de nouvelles idées via les issues
- **Documentation** — Améliorer ou traduire la documentation
- **Implémentation** — Prendre en charge les crates d’implémentation lorsque les conceptions se stabilisent

N’hésitez pas à ouvrir une issue ou à envoyer une pull request.

## 📄 Licence

Ce projet est sous [licence MIT](./LICENSE).

---

**Si ce projet vous est utile, pensez à lui donner une ⭐**
