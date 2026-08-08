# MySQL Backup Service

[![CI](https://github.com/joanfabregat/mysql-s3-backup/actions/workflows/ci.yml/badge.svg)](https://github.com/joanfabregat/mysql-s3-backup/actions/workflows/ci.yml)
[![Build and push Docker image](https://github.com/joanfabregat/mysql-s3-backup/actions/workflows/docker-image.yml/badge.svg)](https://github.com/joanfabregat/mysql-s3-backup/actions/workflows/docker-image.yml)
[![Docker Pulls](https://img.shields.io/docker/pulls/joanfabregat/mysql-s3-backup)](https://hub.docker.com/r/joanfabregat/mysql-s3-backup)
[![Docker Image Size](https://img.shields.io/docker/image-size/joanfabregat/mysql-s3-backup/latest)](https://hub.docker.com/r/joanfabregat/mysql-s3-backup)
[![GitHub release](https://img.shields.io/github/v/release/joanfabregat/mysql-s3-backup)](https://github.com/joanfabregat/mysql-s3-backup/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A containerized solution for automated MySQL database backups to Amazon S3 and S3-compatible storage providers (Backblaze B2, MinIO, etc.).

## Overview

This service provides a reliable way to backup MySQL databases to Amazon S3 or any S3-compatible storage provider (Backblaze B2, MinIO, etc.). It creates compressed database dumps and uploads them to a specified bucket. Retention policies can be configured using bucket lifecycle rules.

## Features

- MySQL database dumping via `mysqldump` (with `--no-tablespaces` flag)
- Optional `--single-transaction` dumps, so backups don't block writes (see [Dump consistency and locking](#dump-consistency-and-locking))
- Support for both TCP and Unix socket connections
- Automatic compression of database dumps using gzip
- Upload to Amazon S3 or any S3-compatible provider (Backblaze B2, MinIO, etc.) via [`s5cmd`](https://github.com/peak/s5cmd)
- Configurable storage class (defaults to STANDARD_IA, can be disabled for non-AWS providers)
- Configurable S3 bucket path prefixing
- Timestamp-based backup naming for easy sorting and identification
- Automatic cleanup of local temporary files
- Retry mechanism for S3 uploads (3 attempts with 5s delay)
- Support for DATABASE_URL connection strings
- Multi-database backup via comma-separated `MYSQL_DATABASE`
- Multi-architecture Docker images (amd64, arm64)

## Requirements

- Docker
- S3-compatible storage bucket (AWS S3, Backblaze B2, MinIO, etc.)
- Credentials with write access to the bucket
- MySQL/MariaDB database

## Configuration

The service is configured using environment variables:

### Database Connection

Configure one of the following:

**Option 1: Using DATABASE_URL**
```
DATABASE_URL=mysql://user:password@hostname:port/database?unix_socket=/path/to/socket
```

**Option 2: Using individual parameters**
```
MYSQL_HOST=hostname
MYSQL_PORT=3306 (optional, defaults to 3306)
MYSQL_USER=username
MYSQL_PASSWORD=password
MYSQL_DATABASE=database (supports comma-separated list, e.g. db1,db2)
MYSQL_SOCKET=/path/to/socket (optional, for Unix socket connections)
MYSQL_SINGLE_TRANSACTION=0 (optional, defaults to 0; see Dump consistency and locking)
```

### S3 / Storage Configuration

```
S3_BUCKET=your-bucket-name
S3_PREFIX=backups/mysql (optional, defaults to root)
S3_ENDPOINT_URL=https://s3.us-west-004.backblazeb2.com (optional, for S3-compatible providers)
S3_STORAGE_CLASS=STANDARD_IA (optional, defaults to STANDARD_IA; set to empty string to disable)
AWS_ACCESS_KEY_ID=your-access-key
AWS_SECRET_ACCESS_KEY=your-secret-key
AWS_DEFAULT_REGION=us-west-1
```

## Dump consistency and locking

By default `mysqldump` runs with `--lock-tables` (part of `--opt`): it takes
`LOCK TABLES … READ LOCAL` over every table in a database and holds it for that
database's entire dump. **Writes to those tables block for as long as the dump
takes** — minutes on a large schema. In exchange, every table in the dump comes
from the same point in time, whatever storage engine it uses.

Setting `MYSQL_SINGLE_TRANSACTION=1` adds `--single-transaction`, which reads
inside a transaction instead of locking. Writes are never blocked, and the backup
user no longer needs `LOCK TABLES`.

The catch: **`--single-transaction` is only a consistent snapshot of
transactional tables.** InnoDB tables are safe. MyISAM, Aria and MEMORY tables
are read outside the transaction, so a write landing mid-dump can tear them —
silently, with nothing in the dump to indicate it. That is why this is opt-in and
off by default: a backup tool should not quietly downgrade what "a valid backup"
means on an image pull.

Accepted values are `1`/`0`, `true`/`false`, `yes`/`no`, `on`/`off`. Anything
else is rejected at startup rather than being silently treated as off.

### Which one to use

| | `MYSQL_SINGLE_TRANSACTION=0` (default) | `MYSQL_SINGLE_TRANSACTION=1` |
|---|---|---|
| Writes during the dump | Blocked | Not blocked |
| InnoDB consistency | Yes | Yes |
| MyISAM / Aria / MEMORY consistency | Yes | **No — can be torn** |
| Grants needed | `SELECT, SHOW VIEW, TRIGGER, LOCK TABLES` | `SELECT, SHOW VIEW, TRIGGER` |

**All-InnoDB deployment:** enable it. There is no downside.

**Mixed engines:** the right fix is usually converting the remaining tables to
InnoDB — InnoDB has supported `FULLTEXT` since MySQL 5.6, which is the common
reason a MyISAM table is still around. Until then, leaving this off trades a
known cost (the write stall) for a known guarantee.

When enabled, the script queries `information_schema` before each dump and logs
any non-InnoDB table it finds, so a mixed schema shows up in the backup log
rather than in a failed restore:

```
Dumping MySQL database: mixeddb
Warning: MYSQL_SINGLE_TRANSACTION is enabled but 'mixeddb' has non-transactional tables:
Warning:   doc_recherche_index (MyISAM)
Warning: these are dumped outside the transaction and a concurrent write can tear them.
Warning: convert them to InnoDB, or unset MYSQL_SINGLE_TRANSACTION to lock instead.
```

The check is advisory: if it cannot run, it warns and the backup proceeds.

Note that **neither mode gives a consistent snapshot across databases.** With a
comma-separated `MYSQL_DATABASE`, each database is dumped by its own `mysqldump`
invocation, so each gets its own lock or transaction window and they do not line
up with each other.

## Installation

Pull the image from Docker Hub:

```bash
docker pull joanfabregat/mysql-s3-backup
```

## Usage

### Docker Run

```bash
docker run \
  -e DATABASE_URL="mysql://user:password@hostname:3306/database" \
  -e S3_BUCKET="my-backup-bucket" \
  -e S3_PREFIX="mysql/daily" \
  -e AWS_ACCESS_KEY_ID="your-access-key" \
  -e AWS_SECRET_ACCESS_KEY="your-secret-key" \
  -e AWS_DEFAULT_REGION="us-west-1" \
  joanfabregat/mysql-s3-backup
```

### Docker Compose

```yaml
services:
  mysql-backup:
    image: joanfabregat/mysql-s3-backup
    environment:
      - MYSQL_HOST=db
      - MYSQL_PORT=3306
      - MYSQL_USER=backup_user
      - MYSQL_PASSWORD=backup_password
      - MYSQL_DATABASE=my_database
      - S3_BUCKET=my-backup-bucket
      - S3_PREFIX=backups/mysql
      - AWS_ACCESS_KEY_ID=your-access-key
      - AWS_SECRET_ACCESS_KEY=your-secret-key
      - AWS_DEFAULT_REGION=us-west-1
```

To back up multiple databases in a single run:

```yaml
services:
  mysql-backup:
    image: joanfabregat/mysql-s3-backup
    environment:
      - MYSQL_HOST=db
      - MYSQL_USER=backup_user
      - MYSQL_PASSWORD=backup_password
      - MYSQL_DATABASE=app_db,analytics_db
      - S3_BUCKET=my-backup-bucket
      - S3_PREFIX=backups/mysql
      - AWS_ACCESS_KEY_ID=your-access-key
      - AWS_SECRET_ACCESS_KEY=your-secret-key
      - AWS_DEFAULT_REGION=us-west-1
```

### Backblaze B2

```yaml
services:
  mysql-backup:
    image: joanfabregat/mysql-s3-backup
    environment:
      - MYSQL_HOST=db
      - MYSQL_USER=backup_user
      - MYSQL_PASSWORD=backup_password
      - MYSQL_DATABASE=my_database
      - S3_BUCKET=my-b2-bucket
      - S3_PREFIX=backups/mysql
      - S3_ENDPOINT_URL=https://s3.us-west-004.backblazeb2.com
      - S3_STORAGE_CLASS=
      - AWS_ACCESS_KEY_ID=your-b2-application-key-id
      - AWS_SECRET_ACCESS_KEY=your-b2-application-key
      - AWS_DEFAULT_REGION=us-west-004
```

## Scheduled Backups

To run scheduled backups, you can:

1. Use Kubernetes CronJob
2. Deploy with Docker and use the host's cron to schedule container execution
3. Implement scheduling logic in your container orchestration system

### Example Kubernetes CronJob

```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: mysql-backup
spec:
  schedule: "0 2 * * *"  # Run daily at 2 AM
  jobTemplate:
    spec:
      template:
        spec:
          containers:
          - name: mysql-backup
            image: joanfabregat/mysql-s3-backup
            env:
            - name: MYSQL_HOST
              value: "db-service"
            - name: MYSQL_USER
              valueFrom:
                secretKeyRef:
                  name: mysql-backup-secrets
                  key: username
            - name: MYSQL_PASSWORD
              valueFrom:
                secretKeyRef:
                  name: mysql-backup-secrets
                  key: password
            - name: MYSQL_DATABASE
              value: "production_db"
            - name: S3_BUCKET
              value: "company-backups"
            - name: S3_PREFIX
              value: "mysql/daily"
            - name: AWS_ACCESS_KEY_ID
              valueFrom:
                secretKeyRef:
                  name: aws-secrets
                  key: access_key
            - name: AWS_SECRET_ACCESS_KEY
              valueFrom:
                secretKeyRef:
                  name: aws-secrets
                  key: secret_key
            - name: AWS_DEFAULT_REGION
              value: "us-west-1"
          restartPolicy: OnFailure
```

## Output and Logging

The service provides logs of the backup process. All output goes to stdout/stderr for container logging systems to capture.

Successful backups will be uploaded to your S3 bucket with filenames in the format:
```
[S3_PREFIX]/YYYY-MM-DDTHHMMSSz.sql.gz
```

When backing up multiple databases (comma-separated `MYSQL_DATABASE`), each dump is uploaded under a database-specific subdirectory:
```
[S3_PREFIX]/[DATABASE]/YYYY-MM-DDTHHMMSSz.sql.gz
```

## Development

```bash
# Lint the shell script
shellcheck backup.sh
```

## Building the Image

```bash
docker build -t joanfabregat/mysql-s3-backup .
```

## Security Considerations

- Use IAM roles when running in AWS environments instead of hardcoded credentials
- Create a dedicated database user with minimal permissions: `SELECT, SHOW VIEW, TRIGGER, LOCK TABLES`, or `SELECT, SHOW VIEW, TRIGGER` with `MYSQL_SINGLE_TRANSACTION=1` (see [Dump consistency and locking](#dump-consistency-and-locking))
- Store sensitive environment variables using appropriate secret management solutions
- Consider encrypting your S3 bucket to protect sensitive data

## Troubleshooting

Common issues:

1. **Connection errors**: Verify database connectivity parameters and network access
2. **Permission denied**: Ensure the MySQL user has proper permissions for dumping
3. **S3 upload failures**: Check AWS credentials and bucket write permissions
4. **Out of space errors**: Ensure enough temporary storage is available
5. **Writes stall while the backup runs**: This is `mysqldump`'s default table
   locking. See [Dump consistency and locking](#dump-consistency-and-locking)
6. **`Access denied … when using LOCK TABLES`**: Either grant `LOCK TABLES` to the
   backup user, or set `MYSQL_SINGLE_TRANSACTION=1`, which does not need it

## License

[MIT License](LICENSE)
