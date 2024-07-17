#!/bin/bash

timestamp=$(date -u +"%Y-%m-%dT%H%M%SZ")
destination="/tmp/$timestamp.sql.gz"
s3_destination="s3://$S3_BUCKET$S3_PREFIX/$timestamp.sql.gz"

# Dump MySQL database
echo "Dumping MySQL database $MYSQL_DATABASE"
mysqldump -h "$MYSQL_HOST" -P "$MYSQL_PORT" -u "$MYSQL_USER" -p"$MYSQL_PASSWORD" --no-tablespaces "$MYSQL_DATABASE" | gzip -c > "$destination"
echo "Dumped MySQL database $MYSQL_DATABASE (file size: $(du -sh "$destination" | cut -f1))"

# Upload to S3
echo "Uploading dump to S3"
aws s3 cp "$destination" "$s3_destination"
echo "Uploaded dump to S3 ($s3_destination)"

# Remove dump
echo "Removing dump"
rm "$destination"