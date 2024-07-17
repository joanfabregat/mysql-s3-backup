# MySQL S3 backup

This repository contains a script to backup a MySQL database to an S3 bucket.

## Usage

The script is intended to be run as a cron job. It will dump the database to a file and upload it to an S3 bucket.

The script is packaged as a Docker image. The image is available on Docker Hub as [`joanfabregat/mysql-s3-backup`](https://hub.docker.com/repository/docker/joanfabregat/mysql-s3-backup/).

## Configuration

The script is configured using environment variables.

| Environment variables   | Description       | Default    |
|-------------------------|-------------------|------------|
| `MYSQL_HOST`            | MySQL host        | *required* |
| `MYSQL_PORT`            | MySQL port        | `3306`     |
| `MYSQL_USER`            | MySQL user        | *required* |
| `MYSQL_PASSWORD`        | MySQL password    | *required* |
| `MYSQL_DATABASE`        | MySQL database    | *required* |
| `S3_BUCKET`             | S3 bucket         | *required* |
| `S3_PREFIX`             | S3 prefix         | `/`        |
| `AWS_ACCESS_KEY_ID`     | AWS access key ID | *required* |
| `AWS_SECRET_ACCESS_KEY` | AWS secret        | *required* |
| `AWS_DEFAULT_REGION`    | AWS region        | *required* |

## Kubernetes CRON

Below is an example of a Kubernetes CRON job that runs the backup every day at midnight.

```yaml
apiVersion: v1
kind: Secret
type: Opaque
metadata:
  name: mysql-s3-backup-cron-secrets
data:
  AWS_SECRET_ACCESS_KEY: "WFhYWFhYWFhYWFhYWFhYWA=="
  MYSQL_PASSWORD: "WFhYWFhYWFhYWFhYWFhYWA=="

---
apiVersion: batch/v1
kind: CronJob
metadata:
  name: mysql-s3-backup-cron-config
spec:
  schedule: "0 0 * * *"
  jobTemplate:
    spec:
      template:
        spec:
          containers:
            - name: mysql-s3-backup-cron
              image: joanfabregat/mysql-s3-backup:latest
              env:
                - name: AWS_ACCESS_KEY_ID
                  value: "your-aws-access-key-id"
                - name: AWS_DEFAULT_REGION
                  value: "eu-west-1"
                - name: S3_BUCKET
                  value: "your-great-bucket"
                - name: S3_PREFIX
                  value: "/some-prefix"
                - name: MYSQL_HOST
                  value: "mysql-service"
                - name: MYSQL_USER
                  value: "root"
                - name: MYSQL_DATABASE
                  value: "your-database"
              envFrom:
                - secretRef:
                    name: mysql-s3-backup-cron-secrets
          restartPolicy: OnFailure

```