# MySQL S3 backup

This repository contains a script to backup a MySQL database to an S3 bucket.

## Usage

## Configuration

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
