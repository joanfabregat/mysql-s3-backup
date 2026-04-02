# mysql-s3-backup

## Code Execution

Never run code locally. Use the `/run-python` skill for all execution (tests, scripts).

## Before Committing

Always run shellcheck before creating a commit:

```bash
shellcheck backup.sh
```

There must be no shellcheck warnings or errors before committing changes.
