---
applyTo: '**'
---

## Codebase Memory MCP

**Use Codebase Memory MCP graph tools first for codebase exploration — before reading files or making code changes.**

This rule applies to every request involving this codebase.

Always call `list_projects` first when you do not already know the project name, then use the `display_name` or exact `name` returned by that tool.

### Workflow

```json
// Step 0 — discover project names
list_projects()

// Step 1 — orient
get_architecture({ "project": "<display_name>" })

// Step 2 — find symbols
search_graph({ "project": "<display_name>", "name_pattern": "<symbol>" })

// Step 3 — trace call chains
trace_path({ "project": "<display_name>", "function_name": "<fn>" })

// Step 4 — read source
get_code_snippet({ "project": "<display_name>", "qualified_name": "<fn>" })

// Step 5 — verify coverage for files you cite
check_index_coverage({ "project": "<display_name>", "paths": ["<file>"] })
```

Only use `read_file` / grep when you need exact raw content or when graph coverage is insufficient.
