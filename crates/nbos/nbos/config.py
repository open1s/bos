"""Config loader wrapper for pybos.

Usage:
    from brainos.config import Config

    cfg = Config()
    cfg.discover()
    data = cfg.load_sync()
    model = data.get("global_model", {}).get("model")
"""

from __future__ import annotations

from typing import Any

from nbos_native import ConfigLoader as PyConfigLoader


class Config:
    def __init__(self, strategy: str = "override") -> None:
        self._inner = PyConfigLoader(strategy)

    def discover(self) -> Config:
        self._inner.discover()
        return self

    def add_file(self, path: str) -> Config:
        self._inner.add_file(path)
        return self

    def add_directory(self, path: str) -> Config:
        self._inner.add_directory(path)
        return self

    def add_inline(self, value: dict) -> Config:
        self._inner.add_inline(value)
        return self

    def reset(self) -> Config:
        self._inner.reset()
        return self

    def load_sync(self) -> dict[str, Any]:
        return self._inner.load_sync()

    def reload_sync(self) -> dict[str, Any]:
        return self._inner.reload_sync()
