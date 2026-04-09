<div align="center">

# 🦀 Claw CR

**El agente de programación de código abierto, construido en Rust. Alternativa a Claude Code.**

[![Estado](https://img.shields.io/badge/status-designing-blue?style=flat-square)](https://github.com/)
[![Idioma](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Origen](https://img.shields.io/badge/origin-Claude_Code_TS-8A2BE2?style=flat-square)](https://docs.anthropic.com/en/docs/claude-code)
[![Licencia](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/)

[English](./README.md) | [简体中文](./README.zh-CN.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Español](./README.es.md) | [Français](./README.fr.md)

🚧 Proyecto en etapa inicial y en desarrollo activo. Aún no está listo para producción.

⭐ Danos una estrella para seguir el proyecto

<img
  src="./docs/assets/screenshot_20260408.png"
  alt="Resumen del proyecto"
  width="100%"
  style="border-radius: 8px; box-shadow: 0 15px 40px rgba(0,0,0,0.25);object-fit:cover;"
/>

</div>

---

## 📖 Tabla de Contenidos

- [Inicio rápido](#-inicio-rápido)
- [Objetivos de diseño](#-objetivos-de-diseño)
- [Contribuir](#-contribuir)
- [Referencias](#-referencias)
- [Licencia](#-licencia)

## 🚀 Inicio rápido

No hay una versión estable todavía; puedes compilar el proyecto desde el código fuente con las instrucciones de abajo.

### Compilar

```bash
git clone https://github.com/claw-cli/claw-code-rust && cd claw-code-rust
cargo build --release

# linux / macos
./target/release/clawcr onboard

# windows
.\target\release\clawcr onboard
```

> [!TIP]
> Asegúrate de tener Rust instalado; se recomienda 1.75+ (vía https://rustup.rs/).

## FAQ

### ¿En qué se diferencia de Claude Code?

Es muy similar a Claude Code en cuanto a capacidades. Estas son las diferencias clave:

- 100 % código abierto
- No depende de un proveedor concreto. ClawCR puede usarse con Claude, OpenAI, z.ai, Qwen, Deepseek o incluso modelos locales. A medida que los modelos evolucionen, la diferencia tenderá a reducirse y los precios a bajar, así que ser agnóstico al proveedor importa.
- Soporte LSP listo para usar
- El soporte TUI ya está implementado
- Está construido con una arquitectura cliente/servidor. Por ejemplo, el core puede ejecutarse localmente en tu máquina mientras se controla de forma remota, por ejemplo desde una app móvil, con la TUI como solo uno de varios clientes posibles.

## 🤝 Contribuir

¡Las contribuciones son bienvenidas! Este proyecto está en una fase temprana de diseño y hay muchas maneras de ayudar:

- **Comentarios de arquitectura** — Revisa el diseño de los crates y propone mejoras
- **Debates RFC** — Propón ideas nuevas mediante issues
- **Documentación** — Ayuda a mejorar o traducir la documentación
- **Implementación** — Toma crates de implementación cuando los diseños se estabilicen

Si quieres, abre un issue o envía un pull request.

## 📄 Licencia

Este proyecto está bajo la [licencia MIT](./LICENSE).

---

**Si este proyecto te resulta útil, considera darle una ⭐**
