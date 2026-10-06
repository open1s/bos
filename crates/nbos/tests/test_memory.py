"""Tests for the high-level Python memory store."""

import json
from pathlib import Path

from nbos import Memory

REPO = Path(__file__).resolve().parents[3]
FIXTURE = REPO / "crates" / "agent" / "tests" / "fixtures" / "memory_ranking.json"


def test_add_assigns_identity():
    memory = Memory()
    item = memory.add("the sky is blue")
    assert item["content"] == "the sky is blue"
    assert item["id"]
    assert item["metadata"] is None
    assert memory.len() == 1
    assert not memory.is_empty()
    assert len(memory) == 1


def test_search_ranks_by_keyword_overlap():
    memory = Memory()
    memory.add("rust ownership and borrowing")
    memory.add("python asyncio event loop")
    memory.add("rust async runtimes")
    hits = memory.search("rust async", 2)
    assert [h["content"] for h in hits] == [
        "rust async runtimes",
        "rust ownership and borrowing",
    ]


def test_empty_query_returns_most_recent_first():
    memory = Memory()
    memory.add("first")
    memory.add("second")
    assert [h["content"] for h in memory.search("  ", 1)] == ["second"]


def test_remove_and_clear():
    memory = Memory()
    first = memory.add("a")
    memory.add("b")
    assert memory.remove(first["id"]) is True
    assert memory.remove(first["id"]) is False
    assert memory.len() == 1
    memory.clear()
    assert memory.is_empty()


def test_to_json_round_trips():
    memory = Memory()
    memory.add("x", {"source": "test"})
    loaded = json.loads(memory.to_json())
    assert loaded[0]["content"] == "x"
    assert loaded[0]["metadata"] == {"source": "test"}


def test_matches_the_shared_rust_fixture():
    data = json.loads(FIXTURE.read_text(encoding="utf-8"))
    memory = Memory()
    for content in data["items"]:
        memory.add(content)
    hits = memory.search(data["query"], data["limit"])
    assert [h["content"] for h in hits] == data["expected"]