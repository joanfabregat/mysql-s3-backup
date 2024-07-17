#!/bin/bash

timestamp=$(date -u +"%Y-%m-%dT%H%M%SZ")

# Dump MySQL database
mysqldump -h $MYSQL_HOST -P $MYSQL_PORT -u $MYSQL_USER -p$MYSQL_PASSWORD --no-tablespaces $MYSQL_DATABASE > /tmp/$MYSQL_DATABASE.sql | gzip -c > /tmp/$MYSQL_DATABASE.sql.gz

# Upload to S3
aws s3 cp /tmp/$MYSQL_DATABASE.sql.gz s3://$S3_BUCKET$S3_PREFIX${timestamp}.sql.gz

# Remove dump
rm /tmp/$MYSQL_DATABASE.sql.gz