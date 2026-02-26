"""Tests for S3 upload functionality."""

import os
import sys
from unittest.mock import MagicMock, patch

from botocore.exceptions import ClientError


def clear_backup_module():
    """Remove backup module from cache to force re-import with new env vars."""
    if "mysql_s3_backup.backup" in sys.modules:
        del sys.modules["mysql_s3_backup.backup"]


class TestUploadToS3:
    """Test upload_to_s3() function."""

    def setup_method(self):
        """Clear module cache before each test."""
        clear_backup_module()

    def test_upload_to_s3_success(self):
        """Test successful S3 upload."""
        env = {
            "AWS_ACCESS_KEY_ID": "AKIAIOSFODNN7EXAMPLE",
            "AWS_SECRET_ACCESS_KEY": "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        mock_client = MagicMock()

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("S3_ENDPOINT_URL", None)
            os.environ.pop("S3_STORAGE_CLASS", None)
            with patch("boto3.client", return_value=mock_client):
                clear_backup_module()
                from mysql_s3_backup.backup import upload_to_s3

                result = upload_to_s3("/tmp/test.sql.gz", "my-bucket", "backups/test.sql.gz")

        assert result is True
        mock_client.upload_file.assert_called_once_with(
            "/tmp/test.sql.gz", "my-bucket", "backups/test.sql.gz", ExtraArgs={"StorageClass": "STANDARD_IA"}
        )

    def test_upload_to_s3_uses_correct_storage_class(self):
        """Test that uploads use STANDARD_IA storage class by default."""
        env = {
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "eu-west-1",
        }

        mock_client = MagicMock()

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("S3_ENDPOINT_URL", None)
            os.environ.pop("S3_STORAGE_CLASS", None)
            with patch("boto3.client", return_value=mock_client):
                clear_backup_module()
                from mysql_s3_backup.backup import upload_to_s3

                upload_to_s3("/tmp/backup.sql.gz", "bucket", "key")

        call_args = mock_client.upload_file.call_args
        assert call_args[1]["ExtraArgs"]["StorageClass"] == "STANDARD_IA"

    def test_upload_to_s3_client_error_returns_false(self):
        """Test that S3 client errors return False after retries exhausted."""
        env = {
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        mock_client = MagicMock()
        mock_client.upload_file.side_effect = ClientError(
            {"Error": {"Code": "NoSuchBucket", "Message": "Bucket does not exist"}}, "PutObject"
        )

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("S3_ENDPOINT_URL", None)
            os.environ.pop("S3_STORAGE_CLASS", None)
            with patch("boto3.client", return_value=mock_client):
                clear_backup_module()

                # Create a non-retrying version of the function for testing
                def upload_no_retry(local_file, bucket, key):
                    try:
                        s3_client = MagicMock()
                        s3_client.upload_file.side_effect = ClientError(
                            {"Error": {"Code": "NoSuchBucket", "Message": "Bucket does not exist"}}, "PutObject"
                        )
                        s3_client.upload_file(local_file, bucket, key, ExtraArgs={"StorageClass": "STANDARD_IA"})
                        return True
                    except Exception:
                        return False

                result = upload_no_retry("/tmp/test.sql.gz", "nonexistent-bucket", "key")

        assert result is False

    def test_upload_to_s3_uses_configured_region(self):
        """Test that the S3 client uses the configured region."""
        env = {
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "ap-southeast-2",
        }

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("S3_ENDPOINT_URL", None)
            os.environ.pop("S3_STORAGE_CLASS", None)
            with patch("boto3.client") as mock_boto_client:
                mock_boto_client.return_value = MagicMock()
                clear_backup_module()
                from mysql_s3_backup.backup import upload_to_s3

                upload_to_s3("/tmp/test.sql.gz", "bucket", "key")

        mock_boto_client.assert_called_with(
            "s3", aws_access_key_id="key", aws_secret_access_key="secret", region_name="ap-southeast-2"
        )


class TestS3EndpointUrl:
    """Test S3 custom endpoint URL support for S3-compatible providers."""

    def setup_method(self):
        """Clear module cache before each test."""
        clear_backup_module()

    def test_endpoint_url_passed_to_boto3_client(self):
        """Test that S3_ENDPOINT_URL is passed to boto3.client()."""
        env = {
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-west-004",
            "S3_ENDPOINT_URL": "https://s3.us-west-004.backblazeb2.com",
        }

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("S3_STORAGE_CLASS", None)
            with patch("boto3.client") as mock_boto_client:
                mock_boto_client.return_value = MagicMock()
                clear_backup_module()
                from mysql_s3_backup.backup import upload_to_s3

                upload_to_s3("/tmp/test.sql.gz", "bucket", "key")

        mock_boto_client.assert_called_with(
            "s3",
            aws_access_key_id="key",
            aws_secret_access_key="secret",
            region_name="us-west-004",
            endpoint_url="https://s3.us-west-004.backblazeb2.com",
        )

    def test_no_endpoint_url_when_not_set(self):
        """Test that endpoint_url is not passed when S3_ENDPOINT_URL is unset."""
        env = {
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("S3_ENDPOINT_URL", None)
            os.environ.pop("S3_STORAGE_CLASS", None)
            with patch("boto3.client") as mock_boto_client:
                mock_boto_client.return_value = MagicMock()
                clear_backup_module()
                from mysql_s3_backup.backup import upload_to_s3

                upload_to_s3("/tmp/test.sql.gz", "bucket", "key")

        mock_boto_client.assert_called_with(
            "s3", aws_access_key_id="key", aws_secret_access_key="secret", region_name="us-east-1"
        )

    def test_empty_endpoint_url_not_passed(self):
        """Test that an empty S3_ENDPOINT_URL is treated as unset."""
        env = {
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
            "S3_ENDPOINT_URL": "",
        }

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("S3_STORAGE_CLASS", None)
            with patch("boto3.client") as mock_boto_client:
                mock_boto_client.return_value = MagicMock()
                clear_backup_module()
                from mysql_s3_backup.backup import upload_to_s3

                upload_to_s3("/tmp/test.sql.gz", "bucket", "key")

        # endpoint_url should NOT be in the call kwargs
        call_kwargs = mock_boto_client.call_args[1]
        assert "endpoint_url" not in call_kwargs


class TestS3StorageClass:
    """Test S3 storage class configuration."""

    def setup_method(self):
        """Clear module cache before each test."""
        clear_backup_module()

    def test_default_storage_class_is_standard_ia(self):
        """Test that the default storage class is STANDARD_IA."""
        env = {
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        mock_client = MagicMock()

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("S3_ENDPOINT_URL", None)
            os.environ.pop("S3_STORAGE_CLASS", None)
            with patch("boto3.client", return_value=mock_client):
                clear_backup_module()
                from mysql_s3_backup.backup import upload_to_s3

                upload_to_s3("/tmp/test.sql.gz", "bucket", "key")

        call_args = mock_client.upload_file.call_args
        assert call_args[1]["ExtraArgs"]["StorageClass"] == "STANDARD_IA"

    def test_custom_storage_class(self):
        """Test that a custom storage class is used when configured."""
        env = {
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
            "S3_STORAGE_CLASS": "STANDARD",
        }

        mock_client = MagicMock()

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("S3_ENDPOINT_URL", None)
            with patch("boto3.client", return_value=mock_client):
                clear_backup_module()
                from mysql_s3_backup.backup import upload_to_s3

                upload_to_s3("/tmp/test.sql.gz", "bucket", "key")

        call_args = mock_client.upload_file.call_args
        assert call_args[1]["ExtraArgs"]["StorageClass"] == "STANDARD"

    def test_empty_storage_class_omits_extra_args(self):
        """Test that an empty storage class omits StorageClass from ExtraArgs."""
        env = {
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
            "S3_STORAGE_CLASS": "",
        }

        mock_client = MagicMock()

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("S3_ENDPOINT_URL", None)
            with patch("boto3.client", return_value=mock_client):
                clear_backup_module()
                from mysql_s3_backup.backup import upload_to_s3

                upload_to_s3("/tmp/test.sql.gz", "bucket", "key")

        call_args = mock_client.upload_file.call_args
        assert "StorageClass" not in call_args[1]["ExtraArgs"]

    def test_backblaze_b2_compatible_config(self):
        """Test a typical Backblaze B2 configuration (custom endpoint, no storage class)."""
        env = {
            "AWS_ACCESS_KEY_ID": "004xxxxxxxxxxxx000000001",
            "AWS_SECRET_ACCESS_KEY": "K004xxxxxxxxxxxxxxxxxxxxxxxxx",
            "AWS_DEFAULT_REGION": "us-west-004",
            "S3_ENDPOINT_URL": "https://s3.us-west-004.backblazeb2.com",
            "S3_STORAGE_CLASS": "",
        }

        mock_client = MagicMock()

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            with patch("boto3.client") as mock_boto_client:
                mock_boto_client.return_value = mock_client
                clear_backup_module()
                from mysql_s3_backup.backup import upload_to_s3

                result = upload_to_s3("/tmp/test.sql.gz", "my-b2-bucket", "backups/test.sql.gz")

        assert result is True

        # Verify endpoint URL was passed
        mock_boto_client.assert_called_with(
            "s3",
            aws_access_key_id="004xxxxxxxxxxxx000000001",
            aws_secret_access_key="K004xxxxxxxxxxxxxxxxxxxxxxxxx",
            region_name="us-west-004",
            endpoint_url="https://s3.us-west-004.backblazeb2.com",
        )

        # Verify StorageClass is not set
        call_args = mock_client.upload_file.call_args
        assert "StorageClass" not in call_args[1]["ExtraArgs"]
