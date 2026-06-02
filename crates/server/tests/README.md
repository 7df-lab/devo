# Testing Skills Functionality

## Unit Tests

Run unit tests for skill protocol types:

```bash
cargo test --package devo-server skills_integration
```

Run protocol contract tests:

```bash
cargo test --package devo-server protocol_contract
```

## Manual Testing

To manually test skills functionality:

1. Create a skill directory structure:

```bash
# Create a skill directory with SKILL.md
mkdir -p /tmp/test-skill
cat > /tmp/test-skill/SKILL.md << 'EOF'
# Test Skill

This is a test skill for manual verification.
EOF
```

2. Start the server with the skill directory in the workspace roots:

```bash
# The server should discover skills from configured roots
# Run your test client connected to the server
```

3. Call the skills/list endpoint:

The endpoint should return the discovered skills in the response:

```json
{
  "skills": [
    {
      "id": "test-skill",
      "name": "test-skill",
      "description": "Skill discovered at /tmp/test-skill/SKILL.md",
      "path": "/tmp/test-skill/SKILL.md",
      "enabled": true,
      "source": { "Workspace": { "cwd": "/tmp/project" } },
      "scope": "repo"
    }
  ]
}
```

4. Call the skills/changed endpoint:

Similar to skills/list, this returns skills when they change:

```json
{
  "skills": [
    {
      "id": "test-skill",
      "name": "test-skill",
      "description": "Skill discovered at /tmp/test-skill/SKILL.md",
      "path": "/tmp/test-skill/SKILL.md",
      "enabled": true,
      "source": { "Workspace": { "cwd": "/tmp/project" } },
      "scope": "repo"
    }
  ]
}
```

5. Call the skills/set_enabled endpoint to persist a path-based toggle:

```json
{
  "path": "/tmp/test-skill/SKILL.md",
  "enabled": false
}
```

## Expected Behaviors

- skills/list returns all discovered skills and honors `force_reload`
- skills/changed forces a refreshed list after workspace changes
- skills/set_enabled persists path-based enablement overrides
- Skills without SKILL.md are not discovered
- Disabled skills (enabled: false) are still listed but not usable
- Skill source and scope indicate whether it came from User, Workspace, System, Admin, or Plugin

## Troubleshooting

- If no skills appear, verify SKILL.md exists in a subdirectory
- Check server logs for discovery errors
- Verify the skill directory is in a configured workspace root
