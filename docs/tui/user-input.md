# Asking for user input in the TUI

From the Python kernel, use `await rlm.request_user_input(questions)` during an active turn:

```python
answer = await rlm.request_user_input([
    {
        "id": "strategy",
        "header": "Strategy",
        "question": "Which strategy should we use?",
        "isOther": True,
        "options": [
            {"label": "Small patch", "description": "Minimize the change"},
            {"label": "Refactor", "description": "Improve the shared implementation"},
        ],
    },
    {"id": "notes", "header": "Notes", "question": "Any additional constraints?"},
])
print(answer["answers"]["strategy"]["answers"])
```

The TUI shows every question in order. Options contain `label` and `description`,
not bare strings. The wire answer contains the original label, not its display
number or description. `isOther: true` enables a custom answer alongside options;
a question without options uses text input. IDs must be unique within the request.
Escape cancels the whole request; incomplete answers are not submitted.

A Native controller must be connected. A cancelled/disconnected request can fail
instead of waiting forever. Questions from a subagent are routed through its
controlling host session.

`isSecret: true` is currently **not supported** by the TUI. It fails explicitly
without opening an unmasked input field. Do not request passwords or API keys via
ordinary questions; use a secure configuration or credential channel instead.
