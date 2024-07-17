#!/bin/bash

# Dump MySQL database
mysqldump -h $MYSQL_HOST -P $MYSQL_PORT -u $MYSQL_USER -p$MYSQL_PASSWORD $MYSQL_DATABASE > /tmp/$MYSQL_DATABASE.sql | gzip -c > /tmp/$MYSQL_DATABASE.sql.gz

# Upload to S3
aws s3 cp /tmp/$MYSQL_DATABASE.sql.gz s3://mybucket/$MYSQL_DATABASE.sql.gz

# Remove dump
rm /tmp/$MYSQL_DATABASE.sql.gz