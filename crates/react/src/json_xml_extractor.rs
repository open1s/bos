use quick_xml::events::Event;
use quick_xml::Reader;
use surfing::extract_json_to_string;

pub struct JsonXmlExtractor;

impl JsonXmlExtractor {
    pub fn new() -> Self {
        Self
    }

    pub fn extract_json(&mut self, input: &str) -> Vec<(usize, usize)> {
        let mut results = Vec::new();

        if let Ok(json_str) = extract_json_to_string(input) {
            if !json_str.is_empty() {
                if let Some(start) = input.find(&json_str) {
                    let end = start + json_str.len();
                    results.push((start, end));
                }
            }
        }
        results
    }

    pub fn extract_part(&mut self, input: &str, start: usize, end: usize) -> String {
        input.get(start..end).unwrap().to_string()
    }

    pub fn extract_xml(&mut self, input: &str) -> Vec<(usize, usize)> {
        let mut results = Vec::new();
        let mut reader = Reader::from_str(input);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let mut pos_stack: Vec<usize> = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(start)) => {
                    let start_pos = reader.buffer_position() as usize;
                    let len = start.len() as usize;
                    if start_pos > 0 {
                        pos_stack.push(start_pos - len - 2);
                    }
                }
                Ok(Event::End(_)) => {
                    let end_pos = reader.buffer_position() as usize;
                    if let Some(start_pos) = pos_stack.pop() {
                        results.push((start_pos, end_pos));
                    }
                }
                Ok(Event::Empty(_)) => {
                    let pos = reader.buffer_position() as usize;
                    if pos > 0 {
                        results.push((pos - 1, pos));
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }

        results
    }
}

impl Default for JsonXmlExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_single_json_object() {
        let mut extractor = JsonXmlExtractor::new();
        let input = "text {\"tool\": \"test\"} after";
        let results = extractor.extract_json(input);

        // println!("{:?}", input.get(results[0].0..results[0].1));

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 5);
        assert_eq!(results[0].1, 21);
    }

    #[test]
    fn extracts_nested_json_objects() {
        let mut extractor = JsonXmlExtractor::new();
        let input = r#"{"outer": {"inner": "value"}}"#;
        let results = extractor.extract_json(input);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], (0, 29));
    }

    #[test]
    fn extracts_json_array() {
        let mut extractor = JsonXmlExtractor::new();
        let input = "text [1,2,3] after";
        let results = extractor.extract_json(input);

        assert!(!results.is_empty());
    }

    #[test]
    fn extracts_single_xml_element() {
        let mut extractor = JsonXmlExtractor::new();
        let input = "text <tool_call>content</tool_call> after";
        let results = extractor.extract_xml(input);

        // println!("{:?}", input.get(results[0].0..results[0].1));

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 5);
        assert_eq!(results[0].1, 35);
    }

    #[test]
    fn extracts_nested_xml_elements() {
        let mut extractor = JsonXmlExtractor::new();
        let input = "<outer><inner>value</inner></outer>";
        let results = extractor.extract_xml(input);

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn extracts_self_closing_xml() {
        let mut extractor = JsonXmlExtractor::new();
        let input = "text <br/> after";
        let results = extractor.extract_xml(input);

        assert_eq!(results.len(), 1);
    }

    #[test]
    fn returns_empty_for_no_json() {
        let mut extractor = JsonXmlExtractor::new();
        let input = "plain text only";
        let results = extractor.extract_json(input);

        assert!(results.is_empty());
    }

    #[test]
    fn returns_empty_for_no_xml() {
        let mut extractor = JsonXmlExtractor::new();
        let input = "plain text only";
        let results = extractor.extract_xml(input);

        assert!(results.is_empty());
    }
}
