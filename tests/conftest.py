"""Pytest configuration and shared fixtures."""

import pytest
from unittest.mock import MagicMock, patch


@pytest.fixture
def mock_env_vars():
    """Fixture that provides common environment variable mocking."""
    env = {
        'MYSQL_HOST': 'localhost',
        'MYSQL_PORT': '3306',
        'MYSQL_USER': 'testuser',
        'MYSQL_PASSWORD': 'testpass',
        'MYSQL_DATABASE': 'testdb',
        'MYSQL_SOCKET': None,
        'S3_BUCKET': 'test-bucket',
        'S3_PREFIX': 'backups',
        'AWS_ACCESS_KEY_ID': 'AKIAIOSFODNN7EXAMPLE',
        'AWS_SECRET_ACCESS_KEY': 'wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY',
        'AWS_DEFAULT_REGION': 'us-east-1',
        'DATABASE_URL': None,
    }
    return env


@pytest.fixture
def mock_s3_client():
    """Fixture that provides a mocked boto3 S3 client."""
    client = MagicMock()
    client.upload_file = MagicMock(return_value=None)
    return client


@pytest.fixture
def mock_subprocess_success():
    """Fixture that provides mocked subprocess for successful mysqldump/gzip."""
    mysqldump_process = MagicMock()
    mysqldump_process.stdout = MagicMock()
    mysqldump_process.wait.return_value = 0

    gzip_process = MagicMock()
    gzip_process.wait.return_value = 0

    return mysqldump_process, gzip_process
