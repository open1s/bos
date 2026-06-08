"""Content classes for multimodal messages (text, images, audio)."""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any, Union


@dataclass
class Binary:
    content_type: str
    source: dict[str, Any]  # {"type": "url"|"base64", "data": str}
    name: str | None = None

    @staticmethod
    def from_base64(content_type: str, data: str, name: str | None = None) -> "Binary":
        return Binary(content_type, {"type": "base64", "data": data}, name)

    @staticmethod
    def from_url(content_type: str, url: str, name: str | None = None) -> "Binary":
        return Binary(content_type, {"type": "url", "data": url}, name)

    def is_image(self) -> bool:
        return self.content_type.startswith("image/")

    def is_audio(self) -> bool:
        return self.content_type.startswith("audio/")

    def url(self) -> str:
        if self.source["type"] == "url":
            return self.source["data"]
        return f"data:{self.content_type};base64,{self.source['data']}"

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {"content_type": self.content_type, "source": self.source}
        if self.name:
            result["name"] = self.name
        return result


@dataclass
class ContentPart:
    """A part of a multimodal message content.

    Can be text or binary.
    """

    part_type: str  # "text" or "binary"
    text_content: str | None = None
    binary_content: Binary | None = None

    @staticmethod
    def text(text: str) -> "ContentPart":
        """Create a text content part."""
        return ContentPart(part_type="text", text_content=text)

    @staticmethod
    def binary(content_type: str, data: str | bytes, name: str | None = None) -> "ContentPart":
        """Create a binary content part with base64 data."""
        if isinstance(data, bytes):
            import base64
            data = base64.b64encode(data).decode("ascii")
        return ContentPart(
            part_type="binary",
            binary_content=Binary.from_base64(content_type, data, name),
        )

    @staticmethod
    def binary_url(content_type: str, url: str, name: str | None = None) -> "ContentPart":
        """Create a binary content part with a URL."""
        return ContentPart(
            part_type="binary",
            binary_content=Binary.from_url(content_type, url, name),
        )

    @staticmethod
    def image(url: str, detail: str | None = None, name: str | None = None) -> "ContentPart":
        """Create an image binary content part."""
        return ContentPart.binary_url("image/url", url, name)

    @staticmethod
    def audio(data: str | bytes, format: str = "mp3") -> "ContentPart":
        """Create an audio binary content part with base64 data."""
        return ContentPart.binary(f"audio/{format}", data)

    @staticmethod
    def audio_url(url: str, format: str = "mp3") -> "ContentPart":
        """Create an audio binary content part with a URL."""
        return ContentPart.binary_url(f"audio/{format}", url)

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {"type": self.part_type}
        if self.text_content is not None:
            result["text"] = self.text_content
        if self.binary_content is not None:
            result["binary"] = self.binary_content.to_dict()
        return result


class Content:
    """Multimodal content for LLM messages.

    Can be simple text or a list of content parts (text, images, audio).

    Examples:
        # Simple text
        Content.text("Hello, world!")

        # Text with image
        Content.parts([
            ContentPart.text("What is in this image?"),
            ContentPart.image("https://example.com/photo.jpg"),
        ])

        # Single image
        Content.image("https://example.com/photo.jpg")
    """

    def __init__(self, parts: list[ContentPart] | None = None, text: str | None = None):
        self._parts = parts
        self._text = text

    @staticmethod
    def text(text: str) -> "Content":
        """Create simple text content."""
        return Content(text=text)

    @staticmethod
    def parts(parts: list[ContentPart]) -> "Content":
        """Create content from multiple parts."""
        return Content(parts=parts)

    @staticmethod
    def image(url: str, detail: str | None = None, name: str | None = None) -> "Content":
        """Create content with a single image."""
        return Content.parts([ContentPart.image(url, detail, name)])

    @staticmethod
    def audio(data: str, format: str = "mp3") -> "Content":
        """Create content with a single audio clip (base64)."""
        return Content.parts([ContentPart.audio(data, format)])

    @staticmethod
    def audio_url(url: str, format: str = "mp3") -> "Content":
        """Create content with a single audio clip (URL)."""
        return Content.parts([ContentPart.audio_url(url, format)])

    def to_json(self) -> str:
        """Convert to JSON string for passing to Rust backend."""
        if self._text is not None:
            return json.dumps({"type": "text", "text": self._text})
        elif self._parts is not None:
            return json.dumps([p.to_dict() for p in self._parts])
        else:
            return json.dumps("")

    def is_multimodal(self) -> bool:
        """Check if this content has multiple parts (not just text)."""
        return self._parts is not None and len(self._parts) > 0


def _parse_content(content: Any) -> str:
    """Parse content from various input types.

    Args:
        content: Can be str, Content, or list of ContentPart

    Returns:
        JSON string suitable for passing to Rust backend
    """
    if isinstance(content, Content):
        return content.to_json()
    elif isinstance(content, str):
        # Check if it's already a JSON string we should pass through
        return content
    elif isinstance(content, list):
        # List of ContentPart
        return json.dumps([p.to_dict() if isinstance(p, ContentPart) else p for p in content])
    else:
        return str(content)