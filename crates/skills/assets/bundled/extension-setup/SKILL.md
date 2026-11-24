---
name: extension-setup
description: Configure a custom model provider, connect or build an MCP server, or install or enable a skill for Devo to use.
---

# Set Up Devo Extensions

Identify the requested artifact and scope. Use the user's chosen source, workspace, and profile. For a repository workflow, use workspace scope. For a capability the user wants Devo to use across projects, use user scope. Resolve the active `DEVO_HOME` (default `~/.devo`) on the host where Devo runs. Use paths and executables accessible from that host.

- **Custom model provider.** Read [the custom provider guide](references/custom-provider.md) to resolve model settings, edit Devo's provider and credential files, and verify the connection after Devo reloads them.
- **MCP server.** Read [the MCP guide](references/mcp.md) to build or configure the server, then verify the connection and expected capabilities.
- **Existing skill.** Read [the skill installation guide](references/skills.md) to install the complete package or enable the exact installed copy.
- **New skill for installation.** Use the bundled `skill-creator` to author and test the package, then apply [the skill installation guide](references/skills.md). Use a skill for reusable instructions or scripts and MCP for a service that needs a client-facing tool or resource interface.

When asked only to author an artifact, return it after testing. When asked to install it or make it available to Devo, carry out the setup with the available tools. Check for an existing entry first. Reuse it if it already matches, and change it only when the user asked to update or replace it. Inspect third-party instructions, scripts, and launch commands as source material before enabling or running them. Keep credentials in the integration's supported credential store or environment and follow the current Devo permissions.

Report the installed name, scope, and path or config file. State the runtime check that passed or the specific action still needed to activate it. Keep credentials out of the report.
