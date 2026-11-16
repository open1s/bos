"""
BrainOS Multimodal Demo (Python)

Shows how to use Content, ContentPart, ContentPart.text/image/audio
to send multimodal messages (text + images + audio) to LLM.

Configuration is loaded from ~/.bos/conf/config.toml or environment variables.
Do NOT hardcode API keys - use config instead.

Config file (~/.bos/conf/config.toml):
    [llm.google]
    model = "nvidia/google/gemma-4-31b-it"
    base_url = "http://127.0.0.1:11436/v1"
    api_key = "your-api-key-here"

Usage:
    python demo_multimodal.py
"""

import asyncio
import base64
import json
import os

from nbos import BrainOS, ConfigLoader
from nbos.content import Content, ContentPart


CAT_IMAGE_1 = "https://download.catpng.net/silver_tabby_cat_on_gray_pillow_beside_clear_glass_window-thumbnail.png"
CAT_IMAGE_2 = "https://download.catpng.net/Three%20cats,%20including%20two%20kittens,%20perched%20on%20a%20stump%20in%20a%20picturesque%20garden%20setting._18887-thumbnail.png"
# From nbos/examples/ -> nbos/ -> crates/ -> bos/ -> docs/audio.wav
AUDIO_FILE = "/Users/gaosg/Projects/bos/docs/audio.wav"


def load_audio_file(audio_path: str) -> str:
    """Load an audio file and return base64 encoded string.

    Args:
        audio_path: Path to audio file (mp3, wav, etc.)

    Returns:
        Base64 encoded audio data
    """
    with open(audio_path, "rb") as f:
        return base64.b64encode(f.read()).decode("utf-8")


def get_model_config():
    """Load model config from config.toml or environment variables.

    Looks for config in this order:
    1. ~/.bos/conf/config.toml under [llm.google]
    2. Environment variables (GOOGLE_API_KEY, NVIDIA_API_KEY, BASE_URL, MODEL)
    3. Defaults for local Gemma endpoint
    """
    loader = ConfigLoader()
    loader.discover()

    config = loader.load_sync() or {}

    google_config = config.get("llm", {}).get("google", {}) if config else {}
    nvidia_config = config.get("llm", {}).get("nvidia", {}) if config else {}
    global_model = config.get("global_model", {}) if config else {}

    model_config = google_config if google_config else nvidia_config if nvidia_config else global_model

    return {
        "model": model_config.get("model", "nvidia/google/gemma-4-31b-it"),
        "base_url": model_config.get("base_url", "http://127.0.0.1:11436/v1"),
        "api_key": model_config.get("api_key", os.environ.get("NVIDIA_API_KEY", "")),
    }


async def demo_text_only():
    """Demo 1: Simple text content (backward compatible)."""
    print("\n" + "=" * 60)
    print("Demo 1: Simple Text Content")
    print("=" * 60)

    model_config = get_model_config()
    async with BrainOS(
        model=model_config["model"],
        base_url=model_config["base_url"],
        api_key=model_config["api_key"],
    ) as brain:
        agent = brain.agent("assistant")

        print("\n📤 Asking: What is Python?")
        result = await agent.ask("What is Python?")
        print(f"📥 Agent: {result[:200]}..." if len(result) > 200 else f"📥 Agent: {result}")


async def demo_content_text():
    """Demo 2: Using Content.text()."""
    print("\n" + "=" * 60)
    print("Demo 2: Using Content.text()")
    print("=" * 60)

    model_config = get_model_config()
    async with BrainOS(
        model=model_config["model"],
        base_url=model_config["base_url"],
        api_key=model_config["api_key"],
    ) as brain:
        agent = brain.agent("assistant")

        content = Content.text("What is 2 + 2?")
        print(f"\n📤 Sending Content.text: {content.to_json()}")
        result = await agent.ask(content)
        print(f"📥 Agent: {result[:200]}..." if len(result) > 200 else f"📥 Agent: {result}")


async def demo_content_single_image():
    """Demo 3: Sending single image using Content.image()."""
    print("\n" + "=" * 60)
    print("Demo 3: Single Image Content (Multimodal)")
    print("=" * 60)

    model_config = get_model_config()
    async with BrainOS(
        model=model_config["model"],
        base_url=model_config["base_url"],
        api_key=model_config["api_key"],
    ) as brain:
        agent = brain.agent("assistant")

        print(f"\n📤 Sending cat image: {CAT_IMAGE_1}")
        content = Content.image(CAT_IMAGE_1)

        print(f"   Content JSON: {json.dumps(content.to_json(), indent=2)[:200]}...")
        result = await agent.ask(content)
        print(f"📥 Agent: {result[:300]}..." if len(result) > 300 else f"📥 Agent: {result}")


async def demo_content_image_with_text():
    """Demo 4: Sending text + image using Content.parts()."""
    print("\n" + "=" * 60)
    print("Demo 4: Text + Image Content (Multimodal)")
    print("=" * 60)

    model_config = get_model_config()
    async with BrainOS(
        model=model_config["model"],
        base_url=model_config["base_url"],
        api_key=model_config["api_key"],
    ) as brain:
        agent = brain.agent("assistant")

        content = Content.parts([
            ContentPart.text("What is in this image? Describe it in detail."),
            ContentPart.image(CAT_IMAGE_1, detail="high"),
        ])

        print(f"\n📤 Sending text + image:")
        print(f"   Image URL: {CAT_IMAGE_1}")
        print(f"   Content JSON: {json.dumps(content.to_json(), indent=2)[:300]}...")

        result = await agent.ask(content)
        print(f"📥 Agent: {result[:400]}..." if len(result) > 400 else f"📥 Agent: {result}")


async def demo_content_multiple_images():
    """Demo 5: Multiple images using Content.parts()."""
    print("\n" + "=" * 60)
    print("Demo 5: Multiple Images Content (Multimodal)")
    print("=" * 60)

    model_config = get_model_config()
    async with BrainOS(
        model=model_config["model"],
        base_url=model_config["base_url"],
        api_key=model_config["api_key"],
    ) as brain:
        agent = brain.agent("assistant")

        content = Content.parts([
            ContentPart.text("I have two images for you. Describe both."),
            ContentPart.image(CAT_IMAGE_1, detail="high"),
            ContentPart.text("And here's a second image:"),
            ContentPart.image(CAT_IMAGE_2, detail="high"),
        ])

        print(f"\n📤 Sending text + 2 images:")
        print(f"   Image 1: {CAT_IMAGE_1}")
        print(f"   Image 2: {CAT_IMAGE_2}")

        result = await agent.ask(content)
        print(f"📥 Agent: {result[:500]}..." if len(result) > 500 else f"📥 Agent: {result}")


async def demo_content_compare_images():
    """Demo 6: Compare two images."""
    print("\n" + "=" * 60)
    print("Demo 6: Compare Two Images")
    print("=" * 60)

    model_config = get_model_config()
    async with BrainOS(
        model=model_config["model"],
        base_url=model_config["base_url"],
        api_key=model_config["api_key"],
    ) as brain:
        agent = brain.agent("assistant")

        content = Content.parts([
            ContentPart.text("Compare these two cat images. What are the similarities and differences?"),
            ContentPart.image(CAT_IMAGE_1),
            ContentPart.image(CAT_IMAGE_2),
        ])

        print(f"\n📤 Comparing two cat images:")
        result = await agent.ask(content)
        print(f"📥 Agent: {result[:500]}..." if len(result) > 500 else f"📥 Agent: {result}")


async def demo_content_audio():
    """Demo 7: Sending text + audio using Content.parts()."""
    print("\n" + "=" * 60)
    print("Demo 7: Text + Audio Content (Multimodal)")
    print("=" * 60)

    model_config = get_model_config()
    async with BrainOS(
        model=model_config["model"],
        base_url=model_config["base_url"],
        api_key=model_config["api_key"],
    ) as brain:
        agent = brain.agent("assistant")

        audio_path = os.environ.get("AUDIO_FILE_PATH", AUDIO_FILE)

        if os.path.exists(audio_path):
            audio_data = load_audio_file(audio_path)
            content = Content.parts([
                ContentPart.text("What does this audio say? Summarize briefly."),
                ContentPart.audio(audio_data, format="m4a"),
            ])
            print(f"\n📤 Sending audio from file: {audio_path}")
            print(f"   Audio data length: {len(audio_data)} bytes")
        else:
            print(f"\n📤 Audio file not found: {audio_path}")
            print(f"   Set AUDIO_FILE_PATH env var to use a different audio file")
            return

        print(f"   Content JSON: {json.dumps(content.to_json())[:150]}...")

        try:
            result = await agent.ask(content)
            print(f"📥 Agent: {result[:300]}..." if len(result) > 300 else f"📥 Agent: {result}")
        except Exception as e:
            print(f"⚠️  Audio request failed: {str(e)[:150]}")

        print(f"   Content JSON: {json.dumps(content.to_json(), indent=2)[:200]}...")


async def demo_streaming():
    """Demo 8: Streaming with text content."""
    print("\n" + "=" * 60)
    print("Demo 8: Streaming Response")
    print("=" * 60)

    model_config = get_model_config()
    async with BrainOS(
        model=model_config["model"],
        base_url=model_config["base_url"],
        api_key=model_config["api_key"],
    ) as brain:
        agent = brain.agent("assistant")

        content = Content.text("Count from 1 to 5, one number per line.")

        print(f"\n📤 Asking (streaming): Count 1 to 5")
        print("📥 Agent streaming: ", end="", flush=True)

        async for token in await agent.stream(content):
            if token:
                print(token, end="", flush=True)
        print()


async def main():
    print("=" * 60)
    print("  BrainOS Multimodal Demo (Python)")
    print("  Model config loaded from ~/.bos/conf/config.toml")
    print("=" * 60)

    # Demo 1: Simple text (backward compatible with existing code)
    await demo_text_only()

    # Demo 2: Content.text
    await demo_content_text()

    # Demo 3: Single image
    await demo_content_single_image()

    # Demo 4: Text + Image (main multimodal demo)
    await demo_content_image_with_text()

    # Demo 5: Multiple images
    await demo_content_multiple_images()

    # Demo 6: Compare two images
    await demo_content_compare_images()

    # Demo 7: Text + Audio
    await demo_content_audio()

    # Demo 8: Streaming
    await demo_streaming()

    print("\n" + "=" * 60)
    print("✅ All demos completed!")
    print("=" * 60)


if __name__ == "__main__":
    asyncio.run(main())