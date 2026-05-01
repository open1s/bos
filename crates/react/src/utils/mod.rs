/// Span representing a matched element with its position, nesting level, and parent span
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub level: usize,
    /// Index of parent span in the spans list (None for root elements)
    pub parent_idx: Option<usize>,
}

impl Span {
    pub fn new(start: usize, end: usize, level: usize) -> Self {
        Self {
            start,
            end,
            level,
            parent_idx: None,
        }
    }

    pub fn start(&self) -> usize {
        self.start
    }
    pub fn end(&self) -> usize {
        self.end
    }

    pub fn level(&self) -> usize {
        self.level
    }

    pub fn parent_idx(&self) -> Option<usize> {
        self.parent_idx
    }

    pub fn is_root(&self) -> bool {
        self.level == 1
    }
}

pub type StreamSpan = Span;

#[derive(Debug, Default)]
pub struct Arena {
    buf: Vec<u8>,
    start: usize,
}

impl Arena {
    pub fn push(&mut self, chunk: &str) {
        self.buf.extend_from_slice(chunk.as_bytes());
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn compact(&mut self, keep_from: usize) {
        if keep_from == 0 {
            return;
        }

        self.buf.drain(..keep_from);
        self.start += keep_from;
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }
}

pub trait StreamExtractor {
    type Item<'a>;

    fn push<'a>(&mut self, chunk: &str) -> Option<Vec<Self::Item<'a>>>;

    fn extract<'a>(&'a self, span: &Span) -> &'a [u8];

    fn extract_str<'a>(&'a self, span: &Span) -> Option<&'a str> {
        std::str::from_utf8(self.extract(span)).ok()
    }

    fn extract_string(&self, span: &Span) -> Option<String> {
        self.extract_str(span).map(|s| s.to_string())
    }

    fn reset(&mut self);
}

#[derive(Debug, Default)]
pub struct JsonExtractor {
    arena: Arena,
    stack: Vec<(u8, usize)>,
    in_string: bool,
    escape: bool,

    scan: usize,
}

impl StreamExtractor for JsonExtractor {
    type Item<'a> = StreamSpan;

    fn push<'a>(&mut self, chunk: &str) -> Option<Vec<Self::Item<'a>>> {
        if self.stack.is_empty() && !self.arena.is_empty() {
            self.arena.compact(self.arena.len());
            self.scan = 0;
        } else if !self.stack.is_empty() {
            let min_pos = self.stack.iter().map(|(_, p)| *p).min().unwrap_or(0);
            if min_pos > 0 {
                self.arena.compact(min_pos);
                for (_, pos) in &mut self.stack {
                    *pos -= min_pos;
                }
                self.scan = self.scan.saturating_sub(min_pos);
            }
        }

        self.arena.push(chunk);

        let mut spans = Vec::new();
        let buf = self.arena.as_slice();

        let mut i = self.scan;

        while i < buf.len() {
            let b = buf[i];

            if self.escape {
                self.escape = false;
                i += 1;
                continue;
            }

            match b {
                b'\\' if self.in_string => {
                    self.escape = true;
                }

                b'"' => {
                    self.in_string = !self.in_string;
                }

                b'{' | b'[' if !self.in_string => {
                    self.stack.push((b, i));
                }

                b'}' | b']' if !self.in_string => {
                    if let Some(&(open, start)) = self.stack.last() {
                        let matched = (open == b'{' && b == b'}') || (open == b'[' && b == b']');

                        if matched {
                            self.stack.pop();

                            let end = i + 1;
                            let level = self.stack.len() + 1;

                            spans.push(Span {
                                start: self.arena.start + start,
                                end: self.arena.start + end,
                                level,
                                parent_idx: None,
                            });
                        }
                    }
                }

                _ => {}
            }

            i += 1;
        }

        self.scan = i;

        // Link parent spans by level (spans sorted deepest-first)
        for i in 0..spans.len() {
            let level = spans[i].level;
            if level > 1 {
                // Parent has level - 1 and comes after current span (deeper first)
                for j in (i + 1)..spans.len() {
                    if spans[j].level == level - 1 {
                        spans[i].parent_idx = Some(j);
                        break;
                    }
                }
            }
        }
        for i in 0..spans.len() {
            let level = spans[i].level;
            if level > 1 {
                for j in 0..i {
                    if spans[j].level == level - 1 {
                        spans[i].parent_idx = Some(j);
                        break;
                    }
                }
            }
        }

        Some(spans)
    }

    fn extract<'a>(&'a self, span: &Span) -> &'a [u8] {
        let start = span.start.saturating_sub(self.arena.start);
        let end = span.end.saturating_sub(self.arena.start);

        &self.arena.as_slice()[start..end]
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Default)]
pub struct XmlExtractor {
    arena: Arena,
    stack: Vec<(Vec<u8>, usize)>,
    in_string: bool,
    escape: bool,
    scan: usize,
}

impl StreamExtractor for XmlExtractor {
    type Item<'a> = StreamSpan;
    fn push<'a>(&mut self, chunk: &str) -> Option<Vec<Self::Item<'a>>> {
        if self.stack.is_empty() && !self.arena.is_empty() {
            self.arena.compact(self.arena.len());
            self.scan = 0;
        } else if !self.stack.is_empty() {
            let min_pos = self.stack.iter().map(|(_, p)| *p).min().unwrap_or(0);
            if min_pos > 0 {
                self.arena.compact(min_pos);
                for (_, pos) in &mut self.stack {
                    *pos -= min_pos;
                }
                self.scan = self.scan.saturating_sub(min_pos);
            }
        }

        self.arena.push(chunk);

        let mut spans = Vec::new();
        let buf = self.arena.as_slice();

        let mut i = self.scan;

        while i < buf.len() {
            let b = buf[i];

            if self.escape {
                self.escape = false;
                i += 1;
                continue;
            }

            if self.in_string {
                if b == b'\\' {
                    self.escape = true;
                } else if b == b'"' {
                    self.in_string = false;
                }
                i += 1;
                continue;
            }

            if b == b'<' {
                let next = buf.get(i + 1).copied();
                if next == Some(b'/') {
                    // Closing tag
                    let name_start = i + 2;
                    let name_end = find_tag_end(buf, name_start);
                    if let Some(name) = get_tag_name(buf, name_start, name_end) {
                        let tag_end = name_end + 1;
                        if let Some(idx) = self.stack.iter().position(|(n, _)| n == &name) {
                            let (_, open_pos) = self.stack[idx];
                            self.stack.remove(idx);
                            let level = self.stack.len() + 1;
                            let parent_idx = self.stack.len();

                            spans.push(Span {
                                start: self.arena.start + open_pos,
                                end: self.arena.start + tag_end,
                                level,
                                parent_idx: None,
                            });

                            if parent_idx > 0 {
                                if let Some(last) = spans.last_mut() {
                                    last.parent_idx = Some(parent_idx - 1);
                                }
                            }
                        }
                    }
                } else if next == Some(b'!') || next == Some(b'?') {
                    // Skip
                } else {
                    let name_start = i + 1;
                    let name_end = find_tag_end(buf, name_start);
                    if let Some(name) = get_tag_name(buf, name_start, name_end) {
                        let is_void = name_end < buf.len() && buf[name_end] == b'/';
                        if is_void {
                            let end = name_end + 2;
                            let level = self.stack.len() + 1;
                            let parent_idx = self.stack.len();

                            spans.push(Span {
                                start: self.arena.start + i,
                                end: self.arena.start + end,
                                level,
                                parent_idx: None,
                            });

                            if parent_idx > 0 {
                                if let Some(last) = spans.last_mut() {
                                    last.parent_idx = Some(parent_idx - 1);
                                }
                            }
                        } else {
                            self.stack.push((name, i));
                        }
                    }
                }
            }

            i += 1;
        }

        self.scan = i;

        // Link parent spans by level (spans sorted deepest-first)
        for i in 0..spans.len() {
            let level = spans[i].level;
            if level > 1 {
                for j in (i + 1)..spans.len() {
                    if spans[j].level == level - 1 {
                        spans[i].parent_idx = Some(j);
                        break;
                    }
                }
            }
        }

        Some(spans)
    }

    fn extract<'a>(&'a self, span: &Span) -> &'a [u8] {
        let start = span.start.saturating_sub(self.arena.start);
        let end = span.end.saturating_sub(self.arena.start);

        &self.arena.as_slice()[start..end]
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Default)]
pub struct MixedExtractor {
    json: JsonExtractor,
    xml: XmlExtractor,
    mode: Option<MixedMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MixedMode {
    Json,
    Xml,
}

impl StreamExtractor for MixedExtractor {
    type Item<'a> = StreamSpan;
    fn push<'a>(&mut self, chunk: &str) -> Option<Vec<Self::Item<'a>>> {
        // Detect mode from first non-whitespace char
        if self.mode.is_none() {
            let first = chunk.trim().chars().next();
            self.mode = match first {
                Some('{') | Some('[') => Some(MixedMode::Json),
                Some('<') => Some(MixedMode::Xml),
                _ => None,
            };
        }

        match self.mode {
            Some(MixedMode::Json) => self.json.push(chunk),
            Some(MixedMode::Xml) => self.xml.push(chunk),
            None => None,
        }
    }

    fn extract<'a>(&'a self, span: &Span) -> &'a [u8] {
        match self.mode {
            Some(MixedMode::Json) => self.json.extract(span),
            Some(MixedMode::Xml) => self.xml.extract(span),
            None => &[],
        }
    }

    fn reset(&mut self) {
        self.json.reset();
        self.xml.reset();
        self.mode = None;
    }
}

impl MixedExtractor {
    pub fn mode(&self) -> Option<&'static str> {
        match self.mode {
            Some(MixedMode::Json) => Some("json"),
            Some(MixedMode::Xml) => Some("xml"),
            None => None,
        }
    }
}

/// A span with its source extractor type
#[derive(Debug, Clone)]
pub struct TypedSpan {
    pub span: Span,
    pub source: SpanSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanSource {
    Json,
    Xml,
}

/// MixedExtractor that can extract both JSON and XML from the same stream
#[derive(Debug, Default)]
pub struct MixedExtractorV2 {
    json: JsonExtractor,
    xml: XmlExtractor,
}

impl StreamExtractor for MixedExtractorV2 {
    type Item<'a> = StreamSpan;
    fn push<'a>(&mut self, chunk: &str) -> Option<Vec<Self::Item<'a>>> {
        self.push_typed(chunk)
            .map(|typed| typed.into_iter().map(|t| t.span).collect::<Vec<_>>())
    }

    fn extract<'a>(&'a self, span: &Span) -> &'a [u8] {
        if let Some(extracted) = self.try_extract(span, SpanSource::Json) {
            return extracted;
        }
        self.try_extract(span, SpanSource::Xml).unwrap_or(&[])
    }

    fn reset(&mut self) {
        self.json.reset();
        self.xml.reset();
    }
}

impl MixedExtractorV2 {
    pub fn extract_typed<'a>(&'a self, typed: &TypedSpan) -> &'a [u8] {
        match typed.source {
            SpanSource::Json => self.json.extract(&typed.span),
            SpanSource::Xml => self.xml.extract(&typed.span),
        }
    }

    fn try_extract<'a>(&'a self, span: &Span, source: SpanSource) -> Option<&'a [u8]> {
        let extracted = match source {
            SpanSource::Json => self.json.extract(span),
            SpanSource::Xml => self.xml.extract(span),
        };
        if extracted.is_empty() {
            None
        } else {
            Some(extracted)
        }
    }

    pub fn push_typed(&mut self, chunk: &str) -> Option<Vec<TypedSpan>> {
        let json_spans = self.json.push(chunk);
        let xml_spans = self.xml.push(chunk);

        let mut typed = Vec::new();

        if let Some(spans) = json_spans {
            let json_parts: Vec<TypedSpan> = spans
                .into_iter()
                .map(|span| TypedSpan {
                    span,
                    source: SpanSource::Json,
                })
                .collect();

            typed.extend(json_parts);
        }

        if let Some(spans) = xml_spans {
            let xml_parts: Vec<TypedSpan> = spans
                .into_iter()
                .map(|span| TypedSpan {
                    span,
                    source: SpanSource::Xml,
                })
                .collect();

            typed.extend(xml_parts);
        }

        typed.sort_by(|a, b| {
            b.span
                .level
                .cmp(&a.span.level)
                .then(a.span.start.cmp(&b.span.start))
        });

        Some(typed)
    }

    pub fn reset_typed(&mut self) {
        self.json.reset();
        self.xml.reset();
    }
}

fn find_tag_end(buf: &[u8], mut i: usize) -> usize {
    while i < buf.len() {
        let b = buf[i];
        if b == b' ' || b == b'/' || b == b'>' {
            return i;
        }
        i += 1;
    }
    i
}

fn get_tag_name(buf: &[u8], start: usize, end: usize) -> Option<Vec<u8>> {
    if start >= end {
        return None;
    }
    let name = &buf[start..end];
    if name.is_empty() {
        return None;
    }
    Some(name.to_vec())
}

#[test]
fn json_nested() {
    let mut ex = JsonExtractor::default();

    let spans = ex.push(r#"{"a":{"b":1}}"#).unwrap();

    assert_eq!(spans.len(), 2);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    let s2 = std::str::from_utf8(ex.extract(&spans[1])).unwrap();

    assert_eq!(s1, r#"{"b":1}"#);
    assert_eq!(s2, r#"{"a":{"b":1}}"#);
}

#[test]
fn json_nested_partial() {
    let mut ex = JsonExtractor::default();

    let spans = ex.push(r#"{"a":{"b":1}"#).unwrap();

    assert_eq!(spans.len(), 1);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();

    assert_eq!(s1, r#"{"b":1}"#);
}

#[test]
fn json_nested_partia_with_plain_text() {
    let mut ex = JsonExtractor::default();

    let spans = ex.push(r#"before {"a":{"b":1} after"#).unwrap();

    assert_eq!(spans.len(), 1);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();

    assert_eq!(s1, r#"{"b":1}"#);
}

#[test]
fn xml_nested() {
    let mut ex = XmlExtractor::default();

    let spans = ex
        .push(r#"<root><child><value>1</value></child></root>"#)
        .unwrap();

    assert_eq!(spans.len(), 3);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    let s2 = std::str::from_utf8(ex.extract(&spans[1])).unwrap();
    let s3 = std::str::from_utf8(ex.extract(&spans[2])).unwrap();

    assert_eq!(s1, r#"<value>1</value>"#);
    assert_eq!(s2, r#"<child><value>1</value></child>"#);
    assert_eq!(s3, r#"<root><child><value>1</value></child></root>"#);
}

#[test]
fn xml_self_closing() {
    let mut ex = XmlExtractor::default();

    let spans = ex.push(r#"<root><item/><item/></root>"#).unwrap();

    assert_eq!(spans.len(), 3);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    let s2 = std::str::from_utf8(ex.extract(&spans[1])).unwrap();
    let s3 = std::str::from_utf8(ex.extract(&spans[2])).unwrap();

    assert_eq!(s1, r#"<item/>"#);
    assert_eq!(s2, r#"<item/>"#);
    assert_eq!(s3, r#"<root><item/><item/></root>"#);
}

#[test]
fn xml_with_attributes() {
    let mut ex = XmlExtractor::default();

    let spans = ex
        .push(r#"<root id="1" class="foo"><child name="x"/></root>"#)
        .unwrap();

    assert_eq!(spans.len(), 1);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();

    assert_eq!(s1, r#"<root id="1" class="foo"><child name="x"/></root>"#);
}

#[test]
fn xml_with_text_content() {
    let mut ex = XmlExtractor::default();

    let spans = ex.push(r#"<root>hello<child/>world</root>"#).unwrap();

    assert_eq!(spans.len(), 2);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    let s2 = std::str::from_utf8(ex.extract(&spans[1])).unwrap();

    assert_eq!(s1, r#"<child/>"#);
    assert_eq!(s2, r#"<root>hello<child/>world</root>"#);
}

#[test]
fn xml_mixed_with_plain_text() {
    let mut ex = XmlExtractor::default();

    let spans = ex.push(r#"before <root><child/></root> after"#).unwrap();

    assert_eq!(spans.len(), 2);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    let s2 = std::str::from_utf8(ex.extract(&spans[1])).unwrap();

    assert_eq!(s1, r#"<child/>"#);
    assert_eq!(s2, r#"<root><child/></root>"#);
}

#[test]
fn xml_streamed() {
    let mut ex = XmlExtractor::default();

    let spans = ex.push(r#"<root><child"#).unwrap();
    assert_eq!(spans.len(), 0);

    let spans = ex.push(r#"/>other</root>"#).unwrap();

    assert_eq!(spans.len(), 1);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();

    assert_eq!(s1, r#"<root><child/>other</root>"#);
}

#[test]
fn xml_deeply_nested() {
    let mut ex = XmlExtractor::default();

    let spans = ex.push(r#"<a><b><c><d/></c></b></a>"#).unwrap();
    assert_eq!(spans.len(), 4);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    let s2 = std::str::from_utf8(ex.extract(&spans[1])).unwrap();
    let s3 = std::str::from_utf8(ex.extract(&spans[2])).unwrap();
    let s4 = std::str::from_utf8(ex.extract(&spans[3])).unwrap();

    assert_eq!(s1, r#"<d/>"#);
    assert_eq!(s2, r#"<c><d/></c>"#);
    assert_eq!(s3, r#"<b><c><d/></c></b>"#);
    assert_eq!(s4, r#"<a><b><c><d/></c></b></a>"#);
}

#[test]
fn xml_comment_and_cdata() {
    let mut ex = XmlExtractor::default();

    let spans = ex.push(r#"<root><!-- comment --><item/></root>"#).unwrap();

    assert_eq!(spans.len(), 2);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    let s2 = std::str::from_utf8(ex.extract(&spans[1])).unwrap();

    assert_eq!(s1, r#"<item/>"#);
    assert_eq!(s2, r#"<root><!-- comment --><item/></root>"#);
}

#[test]
fn xml_reset() {
    let mut ex = XmlExtractor::default();

    let spans = ex.push(r#"<root><item/></root>"#).unwrap();
    assert_eq!(spans.len(), 2);

    ex.reset();

    let spans = ex.push(r#"<new><child/></new>"#).unwrap();
    assert_eq!(spans.len(), 2);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    assert_eq!(s1, r#"<child/>"#);
}

#[test]
fn json_nested_partial_with_stream() {
    let mut ex = JsonExtractor::default();

    let spans = ex.push(r#"before {"a":{"b""#).unwrap();

    assert_eq!(spans.len(), 0);

    let spans = ex.push(r#":1} after"#).unwrap();

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();

    assert_eq!(s1, r#"{"b":1}"#);
}

#[test]
fn json_with_string_values() {
    let mut ex = JsonExtractor::default();

    let spans = ex.push(r#"{"a":"hello","b":"world"}"#).unwrap();

    assert_eq!(spans.len(), 1);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();

    assert_eq!(s1, r#"{"a":"hello","b":"world"}"#);
}

#[test]
fn json_with_escaped_quotes() {
    let mut ex = JsonExtractor::default();

    let spans = ex.push(r#"{"msg":"say \"hello\" world"}"#).unwrap();

    assert_eq!(spans.len(), 1);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();

    assert_eq!(s1, r#"{"msg":"say \"hello\" world"}"#);
}

#[test]
fn json_with_nested_arrays() {
    let mut ex = JsonExtractor::default();

    let spans = ex.push(r#"{"a":[1,2,{"b":3}]}"#).unwrap();

    assert_eq!(spans.len(), 3);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    let s2 = std::str::from_utf8(ex.extract(&spans[1])).unwrap();
    let s3 = std::str::from_utf8(ex.extract(&spans[2])).unwrap();

    assert_eq!(s1, r#"{"b":3}"#);
    assert_eq!(s2, r#"[1,2,{"b":3}]"#);
    assert_eq!(s3, r#"{"a":[1,2,{"b":3}]}"#);
}

#[test]
fn json_mixed_with_text() {
    let mut ex = JsonExtractor::default();

    let spans = ex.push(r#"text {"key":"value"} more text"#).unwrap();

    assert_eq!(spans.len(), 1);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();

    assert_eq!(s1, r#"{"key":"value"}"#);
}

#[test]
fn json_multiple_objects() {
    let mut ex = JsonExtractor::default();

    let spans = ex.push(r#"{"a":1}{"b":2}{"c":3}"#).unwrap();

    assert_eq!(spans.len(), 3);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    let s2 = std::str::from_utf8(ex.extract(&spans[1])).unwrap();
    let s3 = std::str::from_utf8(ex.extract(&spans[2])).unwrap();

    assert_eq!(s1, r#"{"a":1}"#);
    assert_eq!(s2, r#"{"b":2}"#);
    assert_eq!(s3, r#"{"c":3}"#);
}

#[test]
fn json_unicode() {
    let mut ex = JsonExtractor::default();

    let spans = ex.push(r#"{"msg":"你好世界"}"#).unwrap();

    assert_eq!(spans.len(), 1);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();

    assert_eq!(s1, r#"{"msg":"你好世界"}"#);
}

#[test]
fn json_deeply_nested() {
    let mut ex = JsonExtractor::default();

    let spans = ex.push(r#"{"a":{"b":{"c":{"d":1}}}}"#).unwrap();

    assert_eq!(spans.len(), 4);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    let s2 = std::str::from_utf8(ex.extract(&spans[1])).unwrap();
    let s3 = std::str::from_utf8(ex.extract(&spans[2])).unwrap();
    let s4 = std::str::from_utf8(ex.extract(&spans[3])).unwrap();

    assert_eq!(s1, r#"{"d":1}"#);
    assert_eq!(s2, r#"{"c":{"d":1}}"#);
    assert_eq!(s3, r#"{"b":{"c":{"d":1}}}"#);
    assert_eq!(s4, r#"{"a":{"b":{"c":{"d":1}}}}"#);
}

#[test]
fn json_reset() {
    let mut ex = JsonExtractor::default();

    let spans = ex.push(r#"{"a":1}"#).unwrap();
    assert_eq!(spans.len(), 1);

    ex.reset();

    let spans = ex.push(r#"{"b":2}"#).unwrap();
    assert_eq!(spans.len(), 1);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    assert_eq!(s1, r#"{"b":2}"#);
}

#[test]
fn mixed_json() {
    let mut ex = MixedExtractor::default();

    let spans = ex.push(r#"{"key":"value"}"#).unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(ex.mode(), Some("json"));

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    assert_eq!(s1, r#"{"key":"value"}"#);
}

#[test]
fn mixed_xml() {
    let mut ex = MixedExtractor::default();

    let spans = ex.push(r#"<root><item/></root>"#).unwrap();
    assert_eq!(spans.len(), 2);
    assert_eq!(ex.mode(), Some("xml"));

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    assert_eq!(s1, r#"<item/>"#);
}

#[test]
fn mixed_reset() {
    let mut ex = MixedExtractor::default();

    let spans = ex.push(r#"{"a":1}"#).unwrap();
    assert_eq!(spans.len(), 1);

    ex.reset();

    let spans = ex.push(r#"<root/>"#).unwrap();
    assert_eq!(spans.len(), 1);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    assert_eq!(s1, r#"<root/>"#);
}

#[test]
fn mixed_v2_both() {
    let mut ex = MixedExtractorV2::default();

    let typed = ex
        .push_typed(r#"{"key":"value"}<root><item/></root>"#)
        .unwrap();

    assert_eq!(typed.len(), 3);

    let s1 = std::str::from_utf8(ex.extract_typed(&typed[0])).unwrap();
    let s2 = std::str::from_utf8(ex.extract_typed(&typed[1])).unwrap();
    let s3 = std::str::from_utf8(ex.extract_typed(&typed[2])).unwrap();

    // Deepest first: item (level 2), then level 1 sorted by start (json=0, root=15)
    assert_eq!(s1, r#"<item/>"#);
    assert_eq!(s2, r#"{"key":"value"}"#);
    assert_eq!(s3, r#"<root><item/></root>"#);
}

#[test]
fn mixed_v2_json_only() {
    let mut ex = MixedExtractorV2::default();

    let spans = ex.json.push(r#"{"a":1}{"b":2}"#).unwrap();

    assert_eq!(spans.len(), 2);

    let s1 = std::str::from_utf8(ex.extract(&spans[0])).unwrap();
    let s2 = std::str::from_utf8(ex.extract(&spans[1])).unwrap();

    assert_eq!(s1, r#"{"a":1}"#);
    assert_eq!(s2, r#"{"b":2}"#);
}

#[test]
fn mixed_v2_xml_only() {
    let mut ex = MixedExtractorV2::default();

    let typed = ex.push_typed(r#"<root><item/><item/></root>"#).unwrap();

    assert_eq!(typed.len(), 3);

    let s1 = std::str::from_utf8(ex.extract_typed(&typed[0])).unwrap();
    let s2 = std::str::from_utf8(ex.extract_typed(&typed[1])).unwrap();
    let s3 = std::str::from_utf8(ex.extract_typed(&typed[2])).unwrap();

    // Deepest first: level 2 sorted by start, then level 1
    assert_eq!(s1, r#"<item/>"#);
    assert_eq!(s2, r#"<item/>"#);
    assert_eq!(s3, r#"<root><item/><item/></root>"#);
}

#[test]
fn mixed_v2_reset() {
    let mut ex = MixedExtractorV2::default();

    let typed = ex.push_typed(r#"{"a":1}"#).unwrap();
    assert_eq!(typed.len(), 1);

    ex.reset();

    let typed = ex.push_typed(r#"<root/>"#).unwrap();
    assert_eq!(typed.len(), 1);

    let s1 = std::str::from_utf8(ex.extract_typed(&typed[0])).unwrap();
    assert_eq!(s1, r#"<root/>"#);
}

#[test]
fn json_parent_span() {
    let mut ex = JsonExtractor::default();

    let spans = ex.push(r#"{"a":{"b":1}}"#).unwrap();
    assert_eq!(spans.len(), 2);

    let inner = &spans[0];
    let outer = &spans[1];

    assert_eq!(inner.level, 2);
    assert_eq!(outer.level, 1);

    assert!(inner.parent_idx.is_some());
    let parent_idx = inner.parent_idx.unwrap();
    let parent = &spans[parent_idx];
    assert_eq!(parent.start, outer.start);
    assert_eq!(parent.end, outer.end);
    assert_eq!(parent.level, outer.level);
}

#[test]
fn xml_parent_span() {
    let mut ex = XmlExtractor::default();

    let spans = ex.push(r#"<root><item/></root>"#).unwrap();
    assert_eq!(spans.len(), 2);

    let item = &spans[0];
    let root = &spans[1];

    assert_eq!(item.level, 2);
    assert_eq!(root.level, 1);

    assert!(item.parent_idx.is_some());
    let parent_idx = item.parent_idx.unwrap();
    let parent = &spans[parent_idx];
    assert_eq!(parent.start, root.start);
    assert_eq!(parent.end, root.end);
    assert_eq!(parent.level, root.level);
}

#[cfg(test)]
mod nvidia_streaming_tests {
    use super::*;

    #[test]
    fn nvidia_style_text_token() {
        let mut ext = JsonExtractor::default();
        let spans = ext.push(r#"{"choices":[{"delta":{"content":"Hello"}}]}"#);

        // Returns spans for all complete JSON at ALL nesting levels
        assert!(!spans.unwrap().is_empty());
    }

    #[test]
    fn nvidia_style_multiple_choices() {
        let mut ext = JsonExtractor::default();
        let spans =
            ext.push(r#"{"choices":[{"delta":{"content":"A"}},{"delta":{"content":"B"}}]}"#);

        assert!(!spans.unwrap().is_empty());
    }

    #[test]
    fn nvidia_style_tool_call() {
        let mut ext = JsonExtractor::default();
        let spans = ext.push(r#"{"choices":[{"delta":{"tool_calls":[{"id":"c1","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"NYC\"}"}}]}}]}"#);

        assert!(!spans.unwrap().is_empty());
    }

    #[test]
    fn nvidia_style_with_plain_text_prefix() {
        let mut ext = JsonExtractor::default();
        let text =
            "some plain text prefix then json:{\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}";
        let spans = ext.push(text);

        assert!(!spans.unwrap().is_empty());
    }
}
