"""Guards the public nbos namespace.

The package exports both pyo3 classes and pure-Python wrappers. A name in
__all__ that does not resolve breaks ``from nbos import *`` for every user,
and a stale __version__ misleads packaging checks, so both are asserted here.
"""

import re
from pathlib import Path

import nbos

PYPROJECT = Path(__file__).resolve().parents[1] / "pyproject.toml"


def test_star_import_resolves_every_export():
    namespace: dict = {}
    exec("from nbos import *", namespace)  # noqa: S102 - exercising the public API
    for name in nbos.__all__:
        assert name in namespace, f"{name} is in __all__ but does not import"


def test_all_names_resolve_and_are_unique():
    assert len(nbos.__all__) == len(set(nbos.__all__)), "duplicate entries in __all__"
    for name in nbos.__all__:
        assert hasattr(nbos, name), f"__all__ lists {name}, which the module lacks"


def test_version_matches_pyproject():
    match = re.search(r'^version = "([^"]+)"', PYPROJECT.read_text(encoding="utf-8"), re.M)
    assert match, "pyproject.toml has no version"
    assert nbos.__version__ == match.group(1)


def test_wrapper_and_native_bus_types_are_distinct():
    # The plain names stay native for compatibility; the wrappers are aliased.
    assert nbos.PublisherWrapper is not nbos.Publisher
    assert nbos.PyPublisher is nbos.Publisher
    assert nbos.CallableWrapper is not nbos.Callable
