#!/usr/bin/env python3

import logging
import os
import subprocess
import sys
import urllib.parse
from datetime import datetime, UTC

import boto3
import tenacity

# Read config
DATABASE_URL = os.environ.get('DATABASE_URL')
MYSQL_HOST = os.environ.get('MYSQL_HOST')
MYSQL_PORT = os.environ.get('MYSQL_PORT') or '3306'
MYSQL_USER = os.environ.get('MYSQL_USER')
MYSQL_PASSWORD = os.environ.get('MYSQL_PASSWORD')
MYSQL_DATABASE = os.environ.get('MYSQL_DATABASE')
MYSQL_SOCKET = os.environ.get('MYSQL_SOCKET') or None
S3_BUCKET = os.environ.get('S3_BUCKET')
S3_PREFIX = os.environ.get('S3_PREFIX') or '/'
AWS_ACCESS_KEY_ID = os.environ.get('AWS_ACCESS_KEY_ID')
AWS_SECRET_ACCESS_KEY = os.environ.get('AWS_SECRET_ACCESS_KEY')
AWS_DEFAULT_REGION = os.environ.get('AWS_DEFAULT_REGION')

# Setup logging
logger = logging.getLogger()
logger.setLevel(logging.DEBUG)
logger.addHandler(logging.StreamHandler())


def load_config() -> tuple:
    """
    Load MySQL connection parameters from environment variables

    :return: Tuple of connection parameters (host, port, user, password, database, socket)
    """
    if DATABASE_URL:
        try:
            parsed = urllib.parse.urlparse(DATABASE_URL)
            if parsed.scheme != 'mysql':
                logger.error("Error: URL must use mysql:// scheme")
                sys.exit(1)
            query_params = urllib.parse.parse_qs(parsed.query)

            return (
                parsed.hostname,  # Hostname
                (parsed.port or '3306'),  # Port
                parsed.username,  # Username
                parsed.password,  # Password
                parsed.path.lstrip('/'),  # Database
                query_params['unix_socket'][0] if 'unix_socket' in query_params else None  # Unix socket
            )

        except Exception as e:
            logger.exception(f"Error parsing DATABASE_URL: {str(e)}")
            sys.exit(1)
    else:
        return MYSQL_HOST, MYSQL_PORT, MYSQL_USER, MYSQL_PASSWORD, MYSQL_DATABASE, MYSQL_SOCKET


@tenacity.retry(wait=tenacity.wait_fixed(5), stop=tenacity.stop_after_attempt(3))
def upload_to_s3(local_file: str, bucket: str, key: str) -> bool:
    """Upload a file to S3 using boto3 library directly"""
    try:
        # Create an S3 client
        s3_client = boto3.client(
            's3',
            aws_access_key_id=AWS_ACCESS_KEY_ID,
            aws_secret_access_key=AWS_SECRET_ACCESS_KEY,
            region_name=AWS_DEFAULT_REGION
        )

        # Upload the file directly using boto3
        s3_client.upload_file(local_file, bucket, key, ExtraArgs={'StorageClass': 'STANDARD_IA'})

        return True
    except Exception as e:
        logger.exception(f"Error uploading to S3: {str(e)}")
        return False


def main() -> None:
    # Generate timestamp in the same format as bash script
    timestamp = datetime.now(UTC).strftime("%Y-%m-%dT%H%M%SZ")
    destination = f"/tmp/{timestamp}.sql.gz"

    # Read MySQL connection parameters
    host, port, user, password, database, socket = load_config()

    # Validate required parameters
    if not database:
        raise ValueError("Error: Missing required environment variable MYSQL_DATABASE")
    if not user:
        raise ValueError("Error: Missing required environment variable MYSQL_USER")
    if not host and not socket:
        raise ValueError("Error: Missing required environment variable MYSQL_HOST or MYSQL_SOCKET")

    # Build mysqldump command based on connection type
    logger.info(f"Dumping MySQL database: {database}")
    mysqldump_cmd = ['mysqldump', '-u', user, '--no-tablespaces']

    if socket:
        # Use socket connection
        logger.debug(f"Using socket connection: {socket}")
        mysqldump_cmd.extend(['--socket', socket])
    else:
        # Use TCP connection
        logger.debug(f"Using TCP connection: {host}:{port}")
        mysqldump_cmd.extend(['-h', host, '-P', str(port)])

    if password:
        logger.debug("Using password for authentication")
        mysqldump_cmd.append(f'--password={password}')

    mysqldump_cmd.append(database)

    # Open gzip process for piping
    try:
        with open(destination, 'wb') as f:
            logger.debug(f"Starting mysqldump process: {' '.join(mysqldump_cmd)}")
            mysqldump_process = subprocess.Popen(mysqldump_cmd, stdout=subprocess.PIPE)
            gzip_process = subprocess.Popen(['gzip', '-c'], stdin=mysqldump_process.stdout, stdout=f)

            # Close mysqldump's stdout to allow it to receive a SIGPIPE if gzip exits
            mysqldump_process.stdout.close()

            # Wait for processes to complete
            gzip_process.wait()
            mysqldump_exit_code = mysqldump_process.wait()

            if mysqldump_exit_code != 0:
                logger.error("Error: Database dump failed.")
                sys.exit(1)

        # Get file size for reporting
        file_size = subprocess.check_output(['du', '-sh', destination]).decode().split()[0]
        logger.debug(f"Dumped MySQL database (size: {file_size})")

        # Upload to S3
        s3_key = f"{S3_PREFIX.lstrip('/')}/{timestamp}.sql.gz".lstrip('/')
        logger.info(f"Uploading dump to S3: {s3_key}")
        if upload_to_s3(destination, S3_BUCKET, s3_key):
            logger.info("Uploaded successfully.")
        else:
            logger.error("Error: S3 upload failed.")
            sys.exit(1)

        # Remove dump
        logger.debug("Removing local dump")
        os.remove(destination)

    except Exception as e:
        logger.exception(f"Error: {str(e)}")
        sys.exit(1)


if __name__ == "__main__":
    main()
