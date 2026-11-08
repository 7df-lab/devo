# Independent child models

Native recursive agents can select their own model and reasoning level without
changing the parent session. Selection also applies to children of children.

## Discover configured models

```python
models = await rlm.find_models(query="", limit=8)
for model in models:
    print(model.selector, model.name, model.thinking_levels)
```

`query` is a string; an empty query browses the catalog. `limit` is an integer
from 1 to 100, defaults to 8, and cannot be a boolean. Each `RLMModel` has:

- `provider`: the configured provider/connection identifier.
- `id`: the model identifier, which may itself contain slashes.
- `name`: the display name.
- `selector`: the exact `provider/model` selector accepted by `spawn`.
- `thinking_levels`: a list of supported reasoning levels.

Discovery reads enabled, configured, routable connection models. It returns no
credentials and performs no network calls, credential probes, or billing checks.
A catalog result is not a guarantee of live provider health or available credit.
Older hosts returning only the original four fields remain readable; their
`thinking_levels` defaults to an empty list (capabilities are unknown).

## Spawn with independent settings

```python
models = await rlm.find_models(query="", limit=100)
chosen = next((model for model in models if "high" in model.thinking_levels), None)
if chosen is not None:
    reviewer = await rlm.spawn(
        "Review the current diff for concurrency bugs.",
        name="reviewer",
        model=chosen.selector,
        thinking="high",
    )
    print(reviewer.model)
```

The signature remains `spawn(prompt, *, name, model=None, thinking=None)`.
`prompt` and `name` must be nonempty strings; sibling names must be unique.
`model`, when supplied, must be an exact selector, not a display name or an
unqualified model ID. The first slash separates provider from model; remaining
slashes are part of the model ID. `thinking`, when supplied, must be a nonempty
string supported by that model. Invalid types and empty strings are rejected.

Omitting either option, or passing `None`, inherits that option from the
**immediate parent**. Selecting a different model does not reset the inherited
thinking level. If that level is unsupported, admission fails; specify a supported
level from discovery instead. Unknown/disabled/unconfigured model selectors and
unsupported explicit thinking levels also fail, without silently substituting
another model or level. Host failures surface as Python `RuntimeError`.

The model binding is saved on the child session and reused for follow-up turns,
restores, and inherited grandchild settings. Changes to the parent's model or
global defaults do not change an existing child. If a bound connection disappears
or becomes unusable, the child reports an error instead of using a default model.
Direct edits to a connection's endpoint or credentials require reconnecting that
provider so the installed adapter matches the new configuration.

The handle still contains `rlm_child_id`, `name`, `session_dir`, and the effective
`model`. It represents admission, not completion or the child's answer. Child
completion records, explicit agent messages, and shared files deliver results.
The compatibility wire action remains `rlm.run`; no new RPC or tool is needed.
