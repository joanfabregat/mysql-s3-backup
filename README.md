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
- Support for both TCP and Unix socket connections
- Automatic compression of database dumps using gzip
- Upload to Amazon S3 or any S3-compatible provider (Backblaze B2, MinIO, etc.) via `aws` CLI
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
- Create a dedicated database user with minimal permissions (SELECT, LOCK TABLES)
- Store sensitive environment variables using appropriate secret management solutions
- Consider encrypting your S3 bucket to protect sensitive data

## Troubleshooting

Common issues:

1. **Connection errors**: Verify database connectivity parameters and network access
2. **Permission denied**: Ensure the MySQL user has proper permissions for dumping
3. **S3 upload failures**: Check AWS credentials and bucket write permissions
4. **Out of space errors**: Ensure enough temporary storage is available

## License

[MIT License](LICENSE)
