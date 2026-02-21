# Claude Code Instructions

## Before Committing

Always run the full check suite before creating a commit:

```bash
uv run pytest
uv run ruff check .
uv run ruff format --check .
```

All tests must pass and there must be no lint or formatting errors before committing changes.

If formatting issues are found, fix them with:

```bash
uv run ruff format .
```

## On Code Changes

When modifying source code (`src/`) or tests (`tests/`), run the tests and linter to verify the changes:

```bash
uv run pytest
uv run ruff check .
```
