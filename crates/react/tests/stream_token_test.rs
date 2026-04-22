use react::llm::{StreamResponseAccumulator, StreamToken};

fn nvidia_handler(response: &str, start_idx: usize) -> (usize, Option<Vec<StreamToken>>) {
    let remaining = if start_idx < response.len() {
        &response[start_idx..]
    } else {
        return (start_idx, None);
    };

    let mut new_idx = start_idx;
    let mut search_pos = 0;
    let mut all_tokens = Vec::new();

    while search_pos < remaining.len() {
        if let Some(start_json) = remaining[search_pos..].find("{\"") {
            let json_start = search_pos + start_json;
            
            let mut brace_count = 0;
            let mut in_string = false;
            let mut escape_next = false;
            let mut json_end = None;
            for (i, c) in remaining[json_start..].char_indices() {
                if escape_next {
                    escape_next = false;
                    continue;
                }
                if c == '\\' {
                    escape_next = true;
                } else if c == '"' {
                    in_string = !in_string;
                } else if !in_string {
                    if c == '{' {
                        brace_count += 1;
                    } else if c == '}' {
                        brace_count -= 1;
                        if brace_count == 0 {
                            json_end = Some(json_start + i + 1);
                            break;
                        }
                    }
                }
            }
            
            let Some(json_end) = json_end else { break; };
            let json_slice = &remaining[json_start..json_end];

            #[derive(serde::Deserialize)]
            struct Choice {
                delta: Delta,
            }

            #[derive(serde::Deserialize)]
            struct Delta {
                content: Option<String>,
            }

            #[derive(serde::Deserialize)]
            struct Response {
                choices: Vec<Choice>,
            }

            if let Ok(resp) = serde_json::from_str::<Response>(json_slice) {
                for choice in &resp.choices {
                    if let Some(content) = &choice.delta.content {
                        if !content.is_empty() {
                            all_tokens.push(StreamToken::Text(content.clone()));
                        }
                    }
                }
                new_idx = start_idx + json_end;
                search_pos = json_end;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    if !all_tokens.is_empty() {
        return (new_idx, Some(all_tokens));
    }
    (start_idx, None)
}

#[test]
fn test_simple_text() {
    let mut acc = StreamResponseAccumulator::new(nvidia_handler);
    let response = r#"{"choices":[{"delta":{"content":"Hello"}}]}"#;
    let tokens = acc.push(response);
    assert!(tokens.is_some());
    let tokens = tokens.unwrap();
    assert_eq!(tokens.len(), 1);
    if let StreamToken::Text(t) = &tokens[0] {
        assert_eq!(t, "Hello");
    }
}

#[test]
fn test_multiple_json_objects() {
    let mut acc = StreamResponseAccumulator::new(nvidia_handler);
    let response = "text prefix{\"choices\":[{\"delta\":{\"content\":\"A\"}}]}middle{\"choices\":[{\"delta\":{\"content\":\"B\"}}]}";
    let tokens = acc.push(response);
    assert!(tokens.is_some());
    let tokens = tokens.unwrap();
    assert_eq!(tokens.len(), 2);
}

#[test]
fn test_nested_braces() {
    let mut acc = StreamResponseAccumulator::new(nvidia_handler);
    let response = r#"{"choices":[{"delta":{"content":"{nested}"}}]}"#;
    let tokens = acc.push(response);
    assert!(tokens.is_some());
    let tokens = tokens.unwrap();
    assert_eq!(tokens.len(), 1);
}

#[test]
fn test_complex_nested() {
    let mut acc = StreamResponseAccumulator::new(nvidia_handler);
    let response = r#"{"choices":[{"delta":{"content":"{brace}"}}]}"#;
    let tokens = acc.push(response);
    assert!(tokens.is_some());
    let tokens = tokens.unwrap();
    if let StreamToken::Text(t) = &tokens[0] {
        assert_eq!(t, "{brace}");
    }
}

#[test]
fn test_escaped_braces_in_string() {
    let mut acc = StreamResponseAccumulator::new(nvidia_handler);
    let response = r#"{"choices":[{"delta":{"content":"has {brace}"}}]}"#;
    let tokens = acc.push(response);
    assert!(tokens.is_some());
    let tokens = tokens.unwrap();
    if let StreamToken::Text(t) = &tokens[0] {
        assert_eq!(t, "has {brace}");
    }
}