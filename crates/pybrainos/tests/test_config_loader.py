"""Tests for ConfigLoader binding"""
import pytest
import tempfile
import os
from pathlib import Path
from pybrainos import ConfigLoader


class TestConfigLoader:
    """ConfigLoader functionality tests"""

    def test_config_loader_creation(self):
        """Test creating a ConfigLoader instance"""
        loader = ConfigLoader(strategy="override")
        assert loader is not None

    def test_config_loader_with_inline_dict(self):
        """Test adding inline configuration as dictionary"""
        loader = ConfigLoader(strategy="override")
        loader.add_inline({"app": {"name": "test", "debug": True}})
        config = loader.load_sync()
        assert config["app"]["name"] == "test"
        assert config["app"]["debug"] is True

    def test_config_loader_with_nested_dict(self):
        """Test loading nested configuration structures"""
        loader = ConfigLoader(strategy="override")
        loader.add_inline({
            "database": {
                "host": "localhost",
                "port": 5432,
                "credentials": {"user": "admin", "password": "secret"}
            }
        })
        config = loader.load_sync()
        assert config["database"]["host"] == "localhost"
        assert config["database"]["port"] == 5432
        assert config["database"]["credentials"]["user"] == "admin"

    def test_config_loader_with_various_types(self):
        """Test loading configuration with various Python types"""
        loader = ConfigLoader(strategy="override")
        loader.add_inline({
            "string_val": "test",
            "int_val": 42,
            "float_val": 3.14,
            "bool_val": True,
            "null_val": None,
            "list_val": [1, 2, 3],
            "dict_val": {"key": "value"}
        })
        config = loader.load_sync()
        assert config["string_val"] == "test"
        assert config["int_val"] == 42
        assert config["float_val"] == 3.14
        assert config["bool_val"] is True
        assert config["null_val"] is None
        assert config["list_val"] == [1, 2, 3]
        assert config["dict_val"]["key"] == "value"

    def test_config_loader_with_file(self):
        """Test loading configuration from TOML file"""
        with tempfile.NamedTemporaryFile(mode='w', suffix='.toml', delete=False) as f:
            f.write('''
[app]
name = "TestApp"
version = "1.0.0"

[database]
host = "localhost"
port = 5432
''')
            f.flush()
            temp_path = f.name

        try:
            loader = ConfigLoader(strategy="override")
            loader.add_file(temp_path)
            config = loader.load_sync()
            assert config["app"]["name"] == "TestApp"
            assert config["app"]["version"] == "1.0.0"
            assert config["database"]["host"] == "localhost"
            assert config["database"]["port"] == 5432
        finally:
            os.unlink(temp_path)

    def test_config_loader_merge_strategy_override(self):
        """Test override merge strategy"""
        loader = ConfigLoader(strategy="override")
        loader.add_inline({"key": "first"})
        loader.add_inline({"key": "second"})
        config = loader.load_sync()
        assert config["key"] == "second"

    def test_config_loader_merge_strategy_first(self):
        """Test first merge strategy - keeps first value"""
        loader = ConfigLoader(strategy="first")
        loader.add_inline({"key": "first"})
        loader.add_inline({"key": "second"})
        config = loader.load_sync()
        assert config["key"] == "first"

    def test_config_loader_deep_merge(self):
        """Test deep merge strategy for nested objects"""
        loader = ConfigLoader(strategy="deep_merge")
        loader.add_inline({"server": {"host": "localhost", "port": 8080}})
        loader.add_inline({"server": {"debug": True}})
        config = loader.load_sync()
        assert config["server"]["host"] == "localhost"
        assert config["server"]["port"] == 8080
        assert config["server"]["debug"] is True

    def test_config_loader_accumulate_lists(self):
        """Test accumulate merge strategy"""
        loader = ConfigLoader(strategy="accumulate")
        loader.add_inline({"items": ["a", "b"]})
        loader.add_inline({"items": ["c", "d"]})
        config = loader.load_sync()
        # Accumulate should combine the lists
        assert len(config["items"]) >= 2

    def test_config_loader_multiple_files(self):
        """Test loading from multiple TOML files"""
        with tempfile.TemporaryDirectory() as tmpdir:
            # Create first config file
            file1 = Path(tmpdir) / "config1.toml"
            file1.write_text("[app]\nname = \"App1\"")

            # Create second config file
            file2 = Path(tmpdir) / "config2.toml"
            file2.write_text("[app]\nversion = \"1.0\"")

            loader = ConfigLoader(strategy="deep_merge")
            loader.add_file(str(file1))
            loader.add_file(str(file2))
            config = loader.load_sync()
            assert config["app"]["name"] == "App1"
            assert config["app"]["version"] == "1.0"

    def test_config_loader_empty_config(self):
        """Test loading empty configuration"""
        loader = ConfigLoader(strategy="override")
        config = loader.load_sync()
        assert isinstance(config, dict)
