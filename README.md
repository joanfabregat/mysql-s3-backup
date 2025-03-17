# MySQL Backup Service

[![Build and push Docker image](https://github.com/joanfabregat/mysql-s3-backup/actions/workflows/docker-image.yml/badge.svg)](https://github.com/joanfabregat/mysql-s3-backup/actions/workflows/docker-image.yml)

A containerized solution for automated MySQL database backups to Amazon S3.

## Overview

This service provides a reliable way to backup MySQL databases to Amazon S3 storage. It creates compressed database dumps and uploads them to a specified S3 bucket with configurable retention policies.

## Features

- MySQL database dumping via `mysqldump`
- Support for both TCP and Unix socket connections
- Automatic compression of database dumps using gzip
- Direct upload to Amazon S3 with configurable storage class
- Configurable S3 bucket path prefixing
- Timestamp-based backup naming for easy sorting and identification
- Automatic cleanup of local temporary files
- Retry mechanism for S3 uploads
- Comprehensive logging
- Support for DATABASE_URL connection strings

## Requirements

- Docker
- AWS S3 bucket
- AWS credentials with write access to the S3 bucket
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
MYSQL_DATABASE=database
MYSQL_SOCKET=/path/to/socket (optional, for Unix socket connections)
```

### AWS Configuration

```
S3_BUCKET=your-bucket-name
S3_PREFIX=backups/mysql (optional, defaults to root)
AWS_ACCESS_KEY_ID=your-access-key
AWS_SECRET_ACCESS_KEY=your-secret-key
AWS_DEFAULT_REGION=us-west-1
```

## Usage

### Docker Run

```bash
docker run \
  -e DATABASE_URL="mysql://user:password@hostname:3306/database" \
  -e S3_BUCKET="my-backup-bucket" \
  -e S3_PREFIX="mysql/daily" \
  -e AWS_ACCESS_KEY_ID="AKIAXXXXXXXXXXXXXXXX" \
  -e AWS_SECRET_ACCESS_KEY="XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX" \
  -e AWS_DEFAULT_REGION="us-west-1" \
  mysql-backup-service
```

### Docker Compose

```yaml
version: '3'
services:
  mysql-backup:
    image: mysql-backup-service
    environment:
      - MYSQL_HOST=db
      - MYSQL_PORT=3306
      - MYSQL_USER=backup_user
      - MYSQL_PASSWORD=backup_password
      - MYSQL_DATABASE=my_database
      - S3_BUCKET=my-backup-bucket
      - S3_PREFIX=backups/mysql
      - AWS_ACCESS_KEY_ID=AKIAXXXXXXXXXXXXXXXX
      - AWS_SECRET_ACCESS_KEY=XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
      - AWS_DEFAULT_REGION=us-west-1
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
            image: mysql-backup-service
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

The service provides detailed logs of the backup process. All logs are output to stdout/stderr for container logging systems to capture.

Successful backups will be uploaded to your S3 bucket with filenames in the format:
```
[S3_PREFIX]/YYYY-MM-DDTHHMMSSz.sql.gz
```

## Building the Image

```bash
docker build -t mysql-backup-service .
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