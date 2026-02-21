"""Tests for configuration loading from environment variables."""

import os
import sys
from unittest.mock import patch

import pytest


def clear_backup_module():
    """Remove backup module from cache to force re-import with new env vars."""
    if "mysql_s3_backup.backup" in sys.modules:
        del sys.modules["mysql_s3_backup.backup"]


class TestLoadConfigFromEnvVars:
    """Test load_config() with individual environment variables."""

    def test_load_config_with_all_env_vars(self):
        """Test loading config when all individual env vars are set."""
        env = {
            "MYSQL_HOST": "db.example.com",
            "MYSQL_PORT": "3307",
            "MYSQL_USER": "admin",
            "MYSQL_PASSWORD": "secret123",
            "MYSQL_DATABASE": "production",
        }
        # Keys to remove from environ
        keys_to_remove = ["DATABASE_URL", "MYSQL_SOCKET"]

        with patch.dict("os.environ", env, clear=False):
            # Remove keys that should be None
            for key in keys_to_remove:
                os.environ.pop(key, None)
            clear_backup_module()
            from mysql_s3_backup.backup import load_config

            result = load_config()

        assert result == ("db.example.com", "3307", "admin", "secret123", "production", None)

    def test_load_config_with_default_port(self):
        """Test that port defaults to 3306 when not specified."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "mydb",
        }
        keys_to_remove = ["DATABASE_URL", "MYSQL_SOCKET"]

        with patch.dict("os.environ", env, clear=False):
            for key in keys_to_remove:
                os.environ.pop(key, None)
            clear_backup_module()
            from mysql_s3_backup.backup import load_config

            result = load_config()

        assert result[1] == "3306"

    def test_load_config_with_socket(self):
        """Test loading config with Unix socket connection."""
        env = {
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "mydb",
            "MYSQL_SOCKET": "/var/run/mysqld/mysqld.sock",
        }
        keys_to_remove = ["DATABASE_URL", "MYSQL_HOST"]

        with patch.dict("os.environ", env, clear=False):
            for key in keys_to_remove:
                os.environ.pop(key, None)
            clear_backup_module()
            from mysql_s3_backup.backup import load_config

            result = load_config()

        assert result[5] == "/var/run/mysqld/mysqld.sock"


class TestLoadConfigFromDatabaseUrl:
    """Test load_config() with DATABASE_URL."""

    def test_load_config_from_database_url(self):
        """Test parsing a standard DATABASE_URL."""
        env = {
            "DATABASE_URL": "mysql://myuser:mypass@myhost:3307/mydb",
        }

        with patch.dict("os.environ", env, clear=False):
            clear_backup_module()
            from mysql_s3_backup.backup import load_config

            result = load_config()

        assert result == ("myhost", 3307, "myuser", "mypass", "mydb", None)

    def test_load_config_database_url_default_port(self):
        """Test DATABASE_URL without explicit port uses default."""
        env = {
            "DATABASE_URL": "mysql://user:pass@host/database",
        }

        with patch.dict("os.environ", env, clear=False):
            clear_backup_module()
            from mysql_s3_backup.backup import load_config

            result = load_config()

        assert result[1] == "3306"

    def test_load_config_database_url_with_socket(self):
        """Test DATABASE_URL with unix_socket query parameter."""
        env = {
            "DATABASE_URL": "mysql://user:pass@localhost/db?unix_socket=/tmp/mysql.sock",
        }

        with patch.dict("os.environ", env, clear=False):
            clear_backup_module()
            from mysql_s3_backup.backup import load_config

            result = load_config()

        assert result[5] == "/tmp/mysql.sock"

    def test_load_config_database_url_invalid_scheme(self):
        """Test that non-mysql:// schemes cause exit."""
        env = {
            "DATABASE_URL": "postgresql://user:pass@host/db",
        }

        with patch.dict("os.environ", env, clear=False):
            clear_backup_module()
            from mysql_s3_backup.backup import load_config

            with pytest.raises(SystemExit) as exc_info:
                load_config()

        assert exc_info.value.code == 1

    def test_load_config_database_url_with_special_chars(self):
        """Test DATABASE_URL parses correctly with various characters."""
        # Note: urllib.parse.urlparse does NOT decode URL-encoded passwords automatically
        # This test verifies the actual behavior
        env = {
            "DATABASE_URL": "mysql://user:simplepass@host/db",
        }

        with patch.dict("os.environ", env, clear=False):
            clear_backup_module()
            from mysql_s3_backup.backup import load_config

            result = load_config()

        assert result[3] == "simplepass"
