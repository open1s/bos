"""Config loader wrapper for nbos.

The wrapper mirrors the fluent surface of the JavaScript binding: declare
sources with file/directory/inline (or discover), then load() resolves them once
and caches the result for get() and the global_model accessors.

    from nbos.config import Config

    cfg = Config().discover().load()
    model = cfg.model

    cfg = Config.from_inline({"global_model": {"model": "x"}})
"""

from __future__ import annotations

import logging
from typing import Any

from nbos_native import ConfigLoader as PyConfigLoader

DEFAULT_MODEL = "nvidia/meta/llama-3.1-8b-instruct"
DEFAULT_BASE_URL = "https://integrate.api.nvidia.com/v1"

_LOG = logging.getLogger("nbos.config")


class Config:
    def __init__(
        self,
        strategy: str = "override",
        options: dict[str, Any] | None = None,
    ) -> None:
        self._inner = PyConfigLoader(strategy)
        self._loaded = False
        self._config: dict[str, Any] | None = None
        self._options = options or {}

    @classmethod
    def from_file(cls, path: str) -> "Config":
        return cls().file(path).load()

    @classmethod
    def from_directory(cls, path: str) -> "Config":
        return cls().directory(path).load()

    @classmethod
    def from_inline(cls, data: dict[str, Any]) -> "Config":
        return cls().inline(data).load()

    def file(self, path: str) -> "Config":
        self._inner.add_file(path)
        return self

    def directory(self, path: str) -> "Config":
        self._inner.add_directory(path)
        return self

    def inline(self, data: dict[str, Any]) -> "Config":
        self._inner.add_inline(data)
        return self

    # Backwards-compatible aliases; the fluent names above match the JS binding.
    def add_file(self, path: str) -> "Config":
        return self.file(path)

    def add_directory(self, path: str) -> "Config":
        return self.directory(path)

    def add_inline(self, value: dict[str, Any]) -> "Config":
        return self.inline(value)

    def discover(self) -> "Config":
        self._inner.discover()
        return self

    def reset(self) -> "Config":
        self._inner.reset()
        self._loaded = False
        self._config = None
        return self

    def load(self) -> "Config":
        try:
            self._config = self._inner.load_sync()
        except Exception as exc:  # noqa: BLE001 - parity with the JS binding
            _LOG.debug("config load failed, falling back to an empty config: %s", exc)
            self._config = {}
        self._loaded = True
        return self

    def reload(self) -> "Config":
        self._inner.reload_sync()
        return self.load()

    def load_sync(self) -> dict[str, Any]:
        """Resolve and return the config dict without caching (legacy name)."""
        return self._inner.load_sync()

    def reload_sync(self) -> dict[str, Any]:
        """Reset the loader, then resolve and return the config dict (legacy)."""
        self._loaded = False
        self._config = None
        return self._inner.reload_sync()

    def get(self, key: str, default: Any = None) -> Any:
        if not self._loaded:
            self.load()
        value: Any = self._config
        for part in key.split("."):
            if isinstance(value, dict) and part in value:
                value = value[part]
            else:
                return default
        return value

    @property
    def global_model(self) -> dict[str, Any]:
        return self.get("global_model", {})

    @property
    def model(self) -> str:
        return self.get("global_model.model", DEFAULT_MODEL)

    @property
    def base_url(self) -> str:
        return self.get("global_model.base_url", DEFAULT_BASE_URL)

    @property
    def api_key(self) -> str:
        return self.get("global_model.api_key", "")

    @property
    def bus(self) -> dict[str, Any]:
        return self.get("bus", {})

    def to_json(self) -> dict[str, Any]:
        return self._config or {}

    def is_loaded(self) -> bool:
        return self._loaded
