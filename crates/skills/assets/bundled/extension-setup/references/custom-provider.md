# Configure a Custom Model Provider in Devo

Use Python or an available file-editing tool to update the configuration on the host running Devo. In the Python kernel, resolve the active configuration directory from `DEVO_CODING_AGENT_DIR`, then `DEVO_HOME` if set, then `Path.home() / ".devo"`. Register the model first. Change a session selection or default only when the user asks.

## Collect the settings

Use values the user has already provided. Ask only for missing essentials, such as the API base URL, an exact model ID if discovery is inconclusive, or the API key if the endpoint requires one and you have no authorized credential. For example, “What is the API base URL? If you know the model ID, send that too. I can look up the model limits and save the connection and API key for you.”

| User input | JSON field | What it changes |
| --- | --- | --- |
| API base URL | `base_url` | Where Devo sends requests. Include the API prefix required by the endpoint, such as `/v1` for many OpenAI-compatible gateways. Devo appends the operation path. |
| API key | `credential` points to an entry in `auth.json` | Authenticates requests to this endpoint. For a new service that accepts anonymous requests, omit the credential reference. |
| Exact model ID and optional display name | Key under `models` and optional model field `name` | The key identifies the model sent to the API, including any slashes. `name` labels it in the picker. |
| API protocol | `wire_api` | Determines the request format. Choose `openai_chat_completions`, `openai_responses`, or `anthropic_messages` according to the endpoint. Use Chat Completions as the first candidate when a gateway advertises that compatibility. |
| Context length in tokens | `context_window` | Gives Devo the budget for conversation history, instructions, tool data, and room for output. It affects the context meter and when Devo compacts history. |
| Maximum output tokens | `max_tokens` | Caps generation per request. A low cap can cut off replies or tool arguments. Some providers count reasoning tokens toward the cap. |

If the model ID is missing, check the provider's published model list. When the endpoint offers discovery and you have permission to contact it, use Python or an available network tool to query its documented model-list endpoint, such as `GET <base_url>/models` for an OpenAI-compatible service. Keep any credential scoped to the intended endpoint. Use a single clear choice or a documented default, and ask the user to choose when several models would fit.

For unknown limits, use metadata from the exact endpoint, the provider's published specifications, or matching catalog information for the same model and deployment. When still unknown, omit the fields to use Devo's generic budget of 200,000 context tokens and 8,192 output tokens. Explain these as Devo defaults. For a local model or gateway likely to have a smaller window, prefer a clearly identified provisional budget if there is evidence for it. Use positive whole token counts and leave enough context for the prompt. A short inference check can confirm basic connectivity. Use published metadata or a provider error with an explicit limit to establish token caps.

## Edit the files

Add a new user-wide custom provider to `<DEVO_HOME>/custom-providers.json`. For a project-specific override, use `<workspace>/.devo/providers.json`. Inspect existing entries in `providers.json` and `custom-providers.json` at the relevant scopes first. Update an existing provider in the file that owns the setting. For overlapping fields, Devo loads user files before workspace files and loads `custom-providers.json` after `providers.json` at each scope.

Both provider JSON files use this structure. Merge the example into the existing `provider` object and add or update only the intended model. Use the actual model ID and token limits.

```json
{
  "provider": {
    "team-gateway": {
      "name": "Team Gateway",
      "enabled": true,
      "base_url": "https://gateway.example.com/v1",
      "wire_api": "openai_chat_completions",
      "credential": "team-gateway",
      "models": {
        "vendor/example-model": {
          "name": "Example Model",
          "context_window": 32768,
          "max_tokens": 4096
        }
      }
    }
  }
}
```

Parse the existing JSON before editing. Create the file or its parent directory when needed, preserve unrelated providers and models, and write valid UTF-8 JSON. For a programmatic edit, write a temporary file beside the destination and replace the original after validation. Report an existing parse error before overwriting a file whose contents you could not merge.

### Store the API key

Use Python or an available file-editing tool to save a user-provided key or a key from a local source the user authorized. Read the existing user-level `<DEVO_HOME>/auth.json`, create it if needed, and merge the credential entry yourself. Use the real key in place of the example text and set the provider's `credential` field to the matching entry name, including for a workspace provider. When reusing a stored credential, use its existing reference. If a required key is missing, ask the user to supply it or identify an authorized local source. Report credential setup as pending until the real key is available.

```json
{
  "team-gateway": {
    "type": "api_key",
    "key": "<API key supplied by the user>"
  }
}
```

For an existing legacy file with a `credentials` object, add the entry inside that object using `{"kind":"api_key","value":"<key>"}`. Preserve other credentials. Merge the key in memory and write it only to `auth.json`. Keep secrets out of assistant replies, command arguments, provider JSON, repositories, and tool output. When checking the file, report only whether the named entry exists and contains a key. Keep the file readable only by the local user.

### Set a default when requested

The selectable ID for the example is `team-gateway/vendor/example-model`. To make it the user-wide default for new sessions, update the top-level `model` in `<DEVO_HOME>/config.toml`. Use the workspace's `.devo/config.toml` for a workspace default. Keep this assignment outside TOML tables.

```toml
model = "team-gateway/vendor/example-model"
```

## Activate and report

Validate the saved JSON and credential reference. File edits take effect when the backend reads its configuration again. Ask the user to restart the Devo backend if the running process still has the old configuration. After reopening Devo, use `/model` to check that the model appears and select it for the current session if requested. A short prompt on the new model verifies that Devo can route inference. If you only validated files or queried the provider's model list, report the Devo inference check as pending.

Tell the user which files you edited, the selectable model ID, the endpoint and protocol, and whether a credential is ready. Explain how the context window and output cap affect their session, and identify values supplied by the user, published by the provider, or selected as defaults. State the specific activation step if one is still needed.
