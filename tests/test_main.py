"""Tests for main backup orchestration."""

import os
import sys
from unittest.mock import MagicMock, patch

import pytest


def clear_backup_module():
    """Remove backup module from cache to force re-import with new env vars."""
    if "mysql_s3_backup.backup" in sys.modules:
        del sys.modules["mysql_s3_backup.backup"]


class TestMainValidation:
    """Test validation logic in main()."""

    def setup_method(self):
        """Clear module cache before each test."""
        clear_backup_module()

    def test_main_raises_on_missing_database(self):
        """Test that main() raises ValueError when database is missing."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
        }

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("MYSQL_DATABASE", None)
            os.environ.pop("MYSQL_SOCKET", None)
            clear_backup_module()
            from mysql_s3_backup.backup import main

            with pytest.raises(ValueError, match="MYSQL_DATABASE"):
                main()

    def test_main_raises_on_missing_user(self):
        """Test that main() raises ValueError when user is missing."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "3306",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "mydb",
        }

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("MYSQL_USER", None)
            os.environ.pop("MYSQL_SOCKET", None)
            clear_backup_module()
            from mysql_s3_backup.backup import main

            with pytest.raises(ValueError, match="MYSQL_USER"):
                main()

    def test_main_raises_on_missing_host_and_socket(self):
        """Test that main() raises ValueError when both host and socket are missing."""
        env = {
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "mydb",
        }

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("MYSQL_HOST", None)
            os.environ.pop("MYSQL_SOCKET", None)
            clear_backup_module()
            from mysql_s3_backup.backup import main

            with pytest.raises(ValueError, match="MYSQL_HOST or MYSQL_SOCKET"):
                main()


class TestMysqldumpCommand:
    """Test mysqldump command construction."""

    def setup_method(self):
        """Clear module cache before each test."""
        clear_backup_module()

    def test_mysqldump_uses_tcp_connection(self):
        """Test that mysqldump uses TCP connection when host is provided."""
        env = {
            "MYSQL_HOST": "db.example.com",
            "MYSQL_PORT": "3307",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "mydb",
            "S3_BUCKET": "bucket",
            "S3_PREFIX": "prefix",
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        mock_mysqldump = MagicMock()
        mock_mysqldump.stdout = MagicMock()
        mock_mysqldump.wait.return_value = 0

        mock_gzip = MagicMock()
        mock_gzip.wait.return_value = 0

        captured_cmd = []

        def capture_popen(cmd, **kwargs):
            captured_cmd.append(cmd)
            if cmd[0] == "mysqldump":
                return mock_mysqldump
            return mock_gzip

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("MYSQL_SOCKET", None)
            clear_backup_module()

            # Patch before importing to ensure it's applied
            with patch("subprocess.Popen", side_effect=capture_popen):
                with patch("subprocess.check_output", return_value=b"1.0M\t/tmp/file"):
                    with patch("os.remove"):
                        # Now import and run
                        import mysql_s3_backup.backup as backup

                        with patch.object(backup, "upload_to_s3", return_value=True):
                            backup.main()

        mysqldump_cmd = captured_cmd[0]
        assert "-h" in mysqldump_cmd
        assert "db.example.com" in mysqldump_cmd
        assert "-P" in mysqldump_cmd
        assert "3307" in mysqldump_cmd

    def test_mysqldump_uses_socket_connection(self):
        """Test that mysqldump uses socket connection when socket is provided."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "mydb",
            "MYSQL_SOCKET": "/var/run/mysqld/mysqld.sock",
            "S3_BUCKET": "bucket",
            "S3_PREFIX": "prefix",
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        mock_mysqldump = MagicMock()
        mock_mysqldump.stdout = MagicMock()
        mock_mysqldump.wait.return_value = 0

        mock_gzip = MagicMock()
        mock_gzip.wait.return_value = 0

        captured_cmd = []

        def capture_popen(cmd, **kwargs):
            captured_cmd.append(cmd)
            if cmd[0] == "mysqldump":
                return mock_mysqldump
            return mock_gzip

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            clear_backup_module()

            with patch("subprocess.Popen", side_effect=capture_popen):
                with patch("subprocess.check_output", return_value=b"1.0M\t/tmp/file"):
                    with patch("os.remove"):
                        import mysql_s3_backup.backup as backup

                        with patch.object(backup, "upload_to_s3", return_value=True):
                            backup.main()

        mysqldump_cmd = captured_cmd[0]
        assert "--socket" in mysqldump_cmd
        assert "/var/run/mysqld/mysqld.sock" in mysqldump_cmd
        # Should NOT have TCP flags when using socket
        assert "-h" not in mysqldump_cmd

    def test_mysqldump_includes_no_tablespaces_flag(self):
        """Test that mysqldump always includes --no-tablespaces flag."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "mydb",
            "S3_BUCKET": "bucket",
            "S3_PREFIX": "prefix",
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        mock_mysqldump = MagicMock()
        mock_mysqldump.stdout = MagicMock()
        mock_mysqldump.wait.return_value = 0

        mock_gzip = MagicMock()
        mock_gzip.wait.return_value = 0

        captured_cmd = []

        def capture_popen(cmd, **kwargs):
            captured_cmd.append(cmd)
            if cmd[0] == "mysqldump":
                return mock_mysqldump
            return mock_gzip

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("MYSQL_SOCKET", None)
            clear_backup_module()

            with patch("subprocess.Popen", side_effect=capture_popen):
                with patch("subprocess.check_output", return_value=b"1.0M\t/tmp/file"):
                    with patch("os.remove"):
                        import mysql_s3_backup.backup as backup

                        with patch.object(backup, "upload_to_s3", return_value=True):
                            backup.main()

        mysqldump_cmd = captured_cmd[0]
        assert "--no-tablespaces" in mysqldump_cmd


class TestBackupWorkflow:
    """Test the complete backup workflow."""

    def setup_method(self):
        """Clear module cache before each test."""
        clear_backup_module()

    def test_successful_backup_removes_local_file(self):
        """Test that successful backup removes local dump file."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "mydb",
            "S3_BUCKET": "bucket",
            "S3_PREFIX": "prefix",
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        mock_mysqldump = MagicMock()
        mock_mysqldump.stdout = MagicMock()
        mock_mysqldump.wait.return_value = 0

        mock_gzip = MagicMock()
        mock_gzip.wait.return_value = 0

        def mock_popen(cmd, **kwargs):
            if cmd[0] == "mysqldump":
                return mock_mysqldump
            return mock_gzip

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("MYSQL_SOCKET", None)
            clear_backup_module()

            with patch("subprocess.Popen", side_effect=mock_popen):
                with patch("subprocess.check_output", return_value=b"1.0M\t/tmp/file"):
                    with patch("os.remove") as mock_remove:
                        import mysql_s3_backup.backup as backup

                        with patch.object(backup, "upload_to_s3", return_value=True):
                            backup.main()

        mock_remove.assert_called_once()
        # Verify it was called with a path ending in .sql.gz
        call_arg = mock_remove.call_args[0][0]
        assert call_arg.endswith(".sql.gz")
        assert call_arg.startswith("/tmp/")

    def test_mysqldump_failure_exits_with_error(self):
        """Test that mysqldump failure causes exit with code 1."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "mydb",
            "S3_BUCKET": "bucket",
            "S3_PREFIX": "prefix",
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        mock_mysqldump = MagicMock()
        mock_mysqldump.stdout = MagicMock()
        mock_mysqldump.wait.return_value = 1  # Failure exit code

        mock_gzip = MagicMock()
        mock_gzip.wait.return_value = 0

        def mock_popen(cmd, **kwargs):
            if cmd[0] == "mysqldump":
                return mock_mysqldump
            return mock_gzip

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("MYSQL_SOCKET", None)
            with patch("subprocess.Popen", side_effect=mock_popen):
                clear_backup_module()
                from mysql_s3_backup.backup import main

                with pytest.raises(SystemExit) as exc_info:
                    main()

        assert exc_info.value.code == 1

    def test_s3_upload_failure_exits_with_error(self):
        """Test that S3 upload failure causes exit with code 1."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "mydb",
            "S3_BUCKET": "bucket",
            "S3_PREFIX": "prefix",
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        mock_mysqldump = MagicMock()
        mock_mysqldump.stdout = MagicMock()
        mock_mysqldump.wait.return_value = 0

        mock_gzip = MagicMock()
        mock_gzip.wait.return_value = 0

        def mock_popen(cmd, **kwargs):
            if cmd[0] == "mysqldump":
                return mock_mysqldump
            return mock_gzip

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("MYSQL_SOCKET", None)
            clear_backup_module()

            with patch("subprocess.Popen", side_effect=mock_popen):
                with patch("subprocess.check_output", return_value=b"1.0M\t/tmp/file"):
                    import mysql_s3_backup.backup as backup

                    with patch.object(backup, "upload_to_s3", return_value=False):
                        with pytest.raises(SystemExit) as exc_info:
                            backup.main()

        assert exc_info.value.code == 1


class TestS3KeyGeneration:
    """Test S3 key path generation."""

    def setup_method(self):
        """Clear module cache before each test."""
        clear_backup_module()

    def test_s3_key_with_prefix(self):
        """Test that S3 key correctly includes prefix."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "mydb",
            "S3_BUCKET": "bucket",
            "S3_PREFIX": "backups/daily",
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        mock_mysqldump = MagicMock()
        mock_mysqldump.stdout = MagicMock()
        mock_mysqldump.wait.return_value = 0

        mock_gzip = MagicMock()
        mock_gzip.wait.return_value = 0

        captured_s3_key = []

        def mock_popen(cmd, **kwargs):
            if cmd[0] == "mysqldump":
                return mock_mysqldump
            return mock_gzip

        def mock_upload(local_file, bucket, key):
            captured_s3_key.append(key)
            return True

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("MYSQL_SOCKET", None)
            clear_backup_module()

            with patch("subprocess.Popen", side_effect=mock_popen):
                with patch("subprocess.check_output", return_value=b"1.0M\t/tmp/file"):
                    with patch("os.remove"):
                        import mysql_s3_backup.backup as backup

                        with patch.object(backup, "upload_to_s3", side_effect=mock_upload):
                            backup.main()

        assert len(captured_s3_key) == 1
        assert captured_s3_key[0].startswith("backups/daily/")
        assert captured_s3_key[0].endswith(".sql.gz")

    def test_s3_key_with_leading_slash_prefix(self):
        """Test that leading slashes in prefix are handled correctly."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "mydb",
            "S3_BUCKET": "bucket",
            "S3_PREFIX": "/backups/",
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        mock_mysqldump = MagicMock()
        mock_mysqldump.stdout = MagicMock()
        mock_mysqldump.wait.return_value = 0

        mock_gzip = MagicMock()
        mock_gzip.wait.return_value = 0

        captured_s3_key = []

        def mock_popen(cmd, **kwargs):
            if cmd[0] == "mysqldump":
                return mock_mysqldump
            return mock_gzip

        def mock_upload(local_file, bucket, key):
            captured_s3_key.append(key)
            return True

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("MYSQL_SOCKET", None)
            clear_backup_module()

            with patch("subprocess.Popen", side_effect=mock_popen):
                with patch("subprocess.check_output", return_value=b"1.0M\t/tmp/file"):
                    with patch("os.remove"):
                        import mysql_s3_backup.backup as backup

                        with patch.object(backup, "upload_to_s3", side_effect=mock_upload):
                            backup.main()

        # Key should not start with /
        assert not captured_s3_key[0].startswith("/")
        assert captured_s3_key[0].startswith("backups/")


class TestMultipleDatabases:
    """Test multi-database backup support."""

    def setup_method(self):
        """Clear module cache before each test."""
        clear_backup_module()

    def _run_main_with_env(self, env):
        """Helper to run main() with given env and capture S3 keys and mysqldump commands."""
        mock_mysqldump = MagicMock()
        mock_mysqldump.stdout = MagicMock()
        mock_mysqldump.wait.return_value = 0

        mock_gzip = MagicMock()
        mock_gzip.wait.return_value = 0

        captured_s3_keys = []
        captured_cmds = []

        def mock_popen(cmd, **kwargs):
            captured_cmds.append(cmd)
            if cmd[0] == "mysqldump":
                return mock_mysqldump
            return mock_gzip

        def mock_upload(local_file, bucket, key):
            captured_s3_keys.append(key)
            return True

        with patch.dict("os.environ", env, clear=False):
            os.environ.pop("DATABASE_URL", None)
            os.environ.pop("MYSQL_SOCKET", None)
            clear_backup_module()

            with patch("subprocess.Popen", side_effect=mock_popen):
                with patch("subprocess.check_output", return_value=b"1.0M\t/tmp/file"):
                    with patch("os.remove"):
                        import mysql_s3_backup.backup as backup

                        with patch.object(backup, "upload_to_s3", side_effect=mock_upload):
                            backup.main()

        mysqldump_cmds = [cmd for cmd in captured_cmds if cmd[0] == "mysqldump"]
        return captured_s3_keys, mysqldump_cmds

    def test_multiple_databases_produces_separate_dumps(self):
        """Test that multiple databases produce separate dump+upload cycles."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "db1,db2",
            "S3_BUCKET": "bucket",
            "S3_PREFIX": "backups",
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        s3_keys, cmds = self._run_main_with_env(env)

        assert len(s3_keys) == 2
        assert len(cmds) == 2

    def test_multiple_databases_s3_key_format(self):
        """Test that multiple databases use {prefix}/{database}/{timestamp}.sql.gz keys."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "db1,db2",
            "S3_BUCKET": "bucket",
            "S3_PREFIX": "backups",
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        s3_keys, _ = self._run_main_with_env(env)

        assert s3_keys[0].startswith("backups/db1/")
        assert s3_keys[0].endswith(".sql.gz")
        assert s3_keys[1].startswith("backups/db2/")
        assert s3_keys[1].endswith(".sql.gz")

    def test_single_database_preserves_original_key_format(self):
        """Test that single database still uses {prefix}/{timestamp}.sql.gz (backward compat)."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "mydb",
            "S3_BUCKET": "bucket",
            "S3_PREFIX": "backups",
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        s3_keys, _ = self._run_main_with_env(env)

        assert len(s3_keys) == 1
        assert s3_keys[0].startswith("backups/")
        # Should NOT contain database name in path for single-db
        assert "/mydb/" not in s3_keys[0]
        assert s3_keys[0].endswith(".sql.gz")

    def test_multiple_databases_mysqldump_commands(self):
        """Test that each mysqldump command targets the correct database."""
        env = {
            "MYSQL_HOST": "localhost",
            "MYSQL_PORT": "3306",
            "MYSQL_USER": "user",
            "MYSQL_PASSWORD": "pass",
            "MYSQL_DATABASE": "db1,db2",
            "S3_BUCKET": "bucket",
            "S3_PREFIX": "backups",
            "AWS_ACCESS_KEY_ID": "key",
            "AWS_SECRET_ACCESS_KEY": "secret",
            "AWS_DEFAULT_REGION": "us-east-1",
        }

        _, cmds = self._run_main_with_env(env)

        # Database name is the last argument in the mysqldump command
        assert cmds[0][-1] == "db1"
        assert cmds[1][-1] == "db2"
