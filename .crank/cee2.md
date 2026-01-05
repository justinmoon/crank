---
title: Support custom frontmatter fields in task files
priority: 2
status: open
supervision: unsupervised
coding_agent: opencode
created: 2026-01-03
---

## Intent

Allow users to define custom frontmatter fields for their specific needs (e.g., `app` field for monorepo projects) without hardcoding them in crank.

## Spec

- Add configuration file (e.g., `.crank/config.toml` or similar) that defines:
  - Custom fields with types (string, int, list, etc.)
  - Whether fields are required or optional
  - Default values
- Update validation to allow configured custom fields
- Update `crank task schema` to include custom fields when configured
- Update `crank agents.md` to show custom fields in schema output
- Consider: should custom fields be validated strictly or loosely?

### Acceptable Output
- Users can add `app: crank` or other custom fields without validation errors
- Custom field schema is discoverable via `crank task schema`
