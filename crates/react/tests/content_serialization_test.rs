//! Unit tests for content serialization in LLM vendors.
//!
//! These tests verify that Content types (including multimodal with images/audio)
//! are correctly serialized to the format expected by LLM APIs.

use react::llm::types::{Binary, BinarySource, Content, ContentPart};

/// Test that serialize_content in vendor modules correctly handles multimodal content.
/// This is a compile-time verification that the types work correctly.
/// Full integration tests require actual API calls.
#[cfg(test)]
mod content_serialization_tests {
    use super::*;

    fn create_text_content(text: &str) -> Content {
        Content::Text(text.to_string())
    }

    fn create_multimodal_content(text: &str, image_url: &str) -> Content {
        Content::Parts(vec![
            ContentPart::Text {
                text: text.to_string(),
            },
            ContentPart::Binary {
                binary: Binary {
                    content_type: "image/url".to_string(),
                    source: BinarySource::Url(image_url.to_string()),
                    name: Some("high".to_string()),
                },
            },
        ])
    }

    fn create_multimodal_from_json_string(json: &str) -> Content {
        Content::from(json.to_string())
    }

    #[test]
    fn test_content_from_text_string() {
        let content = create_text_content("Hello, world!");
        match content {
            Content::Text(s) => assert_eq!(s, "Hello, world!"),
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn test_content_from_json_string_single_object() {
        let json = r#"{"type":"text","text":"Hello"}"#;
        let content = create_multimodal_from_json_string(json);
        match content {
            Content::Parts(parts) => {
                assert_eq!(parts.len(), 1);
                assert!(matches!(parts[0], ContentPart::Text { .. }));
            }
            _ => panic!("Expected Parts variant"),
        }
    }

    #[test]
    fn test_binary_is_image() {
        let binary = Binary::from_url("image/jpeg", "https://example.com/photo.jpg", None);
        assert!(binary.is_image());
        assert!(!binary.is_audio());
    }

    #[test]
    fn test_binary_is_audio() {
        let binary = Binary::from_url("audio/mp3", "https://example.com/audio.mp3", None);
        assert!(!binary.is_image());
        assert!(binary.is_audio());
    }

    #[test]
    fn test_binary_url() {
        let binary = Binary::from_url("image/png", "https://example.com/image.png", None);
        assert_eq!(binary.url(), "https://example.com/image.png");
    }

    #[test]
    fn test_binary_base64_url() {
        let binary = Binary::from_base64("audio/wav", "SGVsbG8gV29ybGQ=", None);
        assert!(binary.url().starts_with("data:audio/wav;base64,"));
    }

    #[test]
    fn test_content_multimodal_parts() {
        let content = create_multimodal_content(
            "What is in this image?",
            "https://example.com/photo.jpg",
        );
        match content {
            Content::Parts(parts) => {
                assert_eq!(parts.len(), 2);
                let text_part = &parts[0];
                let binary_part = &parts[1];
                assert!(matches!(text_part, ContentPart::Text { .. }));
                assert!(matches!(binary_part, ContentPart::Binary { .. }));
            }
            _ => panic!("Expected Parts variant"),
        }
    }

    #[test]
    fn test_content_serialization_round_trip() {
        let original = create_multimodal_content(
            "Describe this image",
            "https://example.com/photo.jpg",
        );

        // Serialize to JSON string
        let json_str = serde_json::to_string(&original).unwrap();

        // Deserialize back
        let restored: Content = serde_json::from_str(&json_str).unwrap();

        // Should be equal
        assert_eq!(original, restored);
    }

    #[test]
    fn test_content_part_text_serialization() {
        let part = ContentPart::Text {
            text: "Hello".to_string(),
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"text\":\"Hello\""));
    }

    #[test]
    fn test_content_part_binary_serialization() {
        let part = ContentPart::Binary {
            binary: Binary::from_url("image/url", "https://example.com/img.jpg", Some("test".to_string())),
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("\"type\":\"binary\""));
        assert!(json.contains("\"content_type\":\"image/url\""));
        assert!(json.contains("\"url\""));
    }

    #[test]
    fn test_content_from_plain_text_falls_back() {
        // Plain text that is not JSON should become Content::Text
        let content = Content::from("Hello, world!".to_string());
        match content {
            Content::Text(s) => assert_eq!(s, "Hello, world!"),
            _ => panic!("Expected Text variant for plain string"),
        }
    }

    #[test]
    fn test_binary_struct_creation() {
        let binary = Binary {
            content_type: "image/jpeg".to_string(),
            source: BinarySource::Url("https://example.com/photo.jpg".to_string()),
            name: Some("my_image".to_string()),
        };

        assert!(binary.is_image());
        assert!(!binary.is_audio());
        assert_eq!(binary.url(), "https://example.com/photo.jpg");
    }

    #[test]
    fn test_binary_base64_struct_creation() {
        let binary = Binary {
            content_type: "audio/wav".to_string(),
            source: BinarySource::Base64("SGVsbG8gV29ybGQ=".to_string()),
            name: None,
        };

        assert!(!binary.is_image());
        assert!(binary.is_audio());
        assert!(binary.url().starts_with("data:audio/wav;base64,"));
    }
}