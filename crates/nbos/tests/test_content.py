"""Covers the multimodal content classes.

The content helpers are pure Python, but nothing exercised them: their JSON
shape is what reaches the Rust backend, and the bindings disagreed on it
(Python emitted image/url where JS emitted image/jpeg). These tests pin the
shape, and crates/jsbos/test/content.test.js asserts the same shapes for JS.
"""

import json

from nbos.content import Binary, Content, ContentPart


def test_binary_from_url_keeps_the_url_source():
    binary = Binary.from_url("image/png", "https://example.com/a.png", "a.png")
    assert binary.content_type == "image/png"
    assert binary.source == {"type": "url", "data": "https://example.com/a.png"}
    assert binary.name == "a.png"
    assert binary.url() == "https://example.com/a.png"


def test_binary_from_base64_builds_a_data_url():
    binary = Binary.from_base64("image/png", "QUJD", None)
    assert binary.url() == "data:image/png;base64,QUJD"
    assert binary.is_image()
    assert not binary.is_audio()


def test_binary_to_dict_omits_an_absent_name():
    assert Binary.from_url("image/png", "https://example.com/a.png").to_dict() == {
        "content_type": "image/png",
        "source": {"url": "https://example.com/a.png"},
    }


def test_content_part_text_shape():
    assert ContentPart.text("hello").to_dict() == {"type": "text", "text": "hello"}


def test_content_part_image_matches_the_js_content_type():
    part = ContentPart.image("https://example.com/photo.jpg")
    assert part.to_dict() == {
        "type": "binary",
        "binary": {
            "content_type": "image/jpeg",
            "source": {"url": "https://example.com/photo.jpg"},
        },
    }


def test_content_part_binary_encodes_bytes_as_base64():
    part = ContentPart.binary("audio/wav", b"ABC")
    assert part.to_dict()["binary"]["source"] == {"base64": "QUJD"}


def test_content_part_audio_uses_an_audio_content_type():
    assert ContentPart.audio("QUJD", "mp3").to_dict()["binary"]["content_type"] == "audio/mp3"
    assert ContentPart.audio_url("https://example.com/a.mp3").to_dict()["binary"]["source"] == {
        "url": "https://example.com/a.mp3"
    }


def test_content_text_is_not_multimodal():
    content = Content.text("hello")
    assert json.loads(content.to_json()) == {"type": "text", "text": "hello"}
    assert not content.is_multimodal()


def test_content_parts_serializes_an_array():
    content = Content.parts(
        [ContentPart.text("describe"), ContentPart.image("https://example.com/photo.jpg")]
    )
    assert content.is_multimodal()
    parts = json.loads(content.to_json())
    assert [p["type"] for p in parts] == ["text", "binary"]
    assert parts[1]["binary"]["content_type"] == "image/jpeg"


def test_content_image_wraps_a_single_part():
    parts = json.loads(Content.image("https://example.com/photo.jpg").to_json())
    assert len(parts) == 1
    assert parts[0]["binary"]["content_type"] == "image/jpeg"