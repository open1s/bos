"""In-process long-term memory shared by the high-level Python API.

Mirrors ``agent::memory::InMemoryMemory``: text is tokenized into lowercase
alphanumeric runs, an item scores one point per query token it contains, and
ties break toward the more recently added item.
"""

from __future__ import annotations

import json
import re
import time
import uuid
from typing import Any

__all__ = ["Memory"]


def _tokenize(text: str) -> list[str]:
    # Split on anything that is not a letter or digit, matching Rust's
    # char::is_alphanumeric; replace underscores that \w would otherwise keep.
    return [t for t in re.split(r"[^\w]+", text.lower().replace("_", " ")) if t]


class Memory:
    """A keyword-ranked, in-process memory store."""

    def __init__(self) -> None:
        self._items: list[dict[str, Any]] = []

    def add(self, content: str, metadata: Any = None) -> dict[str, Any]:
        """Store ``content`` and return the stored item."""
        item = {
            "id": str(uuid.uuid4()),
            "content": str(content),
            "metadata": metadata,
            "created_at_ms": int(time.time() * 1000),
        }
        self._items.append(item)
        return dict(item)

    def all(self) -> list[dict[str, Any]]:
        """Return every item, oldest first."""
        return [dict(item) for item in self._items]

    def search(self, query: str, limit: int = 5) -> list[dict[str, Any]]:
        """Return up to ``limit`` items relevant to ``query``, best first."""
        if limit <= 0:
            return []
        tokens = _tokenize(str(query))
        if not tokens:
            return [dict(item) for item in reversed(self._items)][:limit]
        scored: list[tuple[int, dict[str, Any]]] = []
        for item in self._items:
            content_tokens = set(_tokenize(item["content"]))
            score = sum(1 for token in tokens if token in content_tokens)
            if score:
                scored.append((score, item))
        scored.sort(key=lambda pair: (-pair[0], -pair[1]["created_at_ms"]))
        return [dict(item) for _, item in scored[:limit]]

    def remove(self, id: str) -> bool:
        """Remove the item with ``id``, returning whether it existed."""
        before = len(self._items)
        self._items = [item for item in self._items if item["id"] != id]
        return len(self._items) != before

    def clear(self) -> None:
        """Remove every item."""
        self._items.clear()

    def save(self, path: str) -> int:
        """Write every item to ``path`` as JSON lines and return the count.

        The layout matches the Rust ``FileMemory``, so files are portable
        between the core and this binding.
        """
        with open(path, "w", encoding="utf-8") as handle:
            for item in self._items:
                handle.write(json.dumps(item) + "\n")
        return len(self._items)

    @classmethod
    def load(cls, path: str) -> "Memory":
        """Read items from a JSON-lines ``path`` written by :meth:`save`."""
        memory = cls()
        with open(path, encoding="utf-8") as handle:
            for line in handle:
                line = line.strip()
                if line:
                    memory._items.append(json.loads(line))
        return memory

    def len(self) -> int:
        """Number of stored items."""
        return len(self._items)

    def is_empty(self) -> bool:
        """Whether the store holds no items."""
        return not self._items

    def to_json(self) -> str:
        """Serialize every item to a JSON array."""
        return json.dumps(self.all())

    def recall_block(self, query: str, limit: int = 5) -> str:
        """Format up to ``limit`` relevant items as a prompt preamble."""
        hits = self.search(query, limit)
        if not hits:
            return ""
        return "Relevant memory:" + "".join("\n- " + h["content"] for h in hits)

    def __len__(self) -> int:
        return len(self._items)