//! SSE（Server-Sent Events）增量解析器（P11，ADR-013）。
//!
//! Provider 管线用 reqwest `bytes_stream` 拉取响应体，按任意大小的字节块
//! 喂给 [`SseParser`]，解析出完整事件（`data:` / `event:` 字段）。
//! 实现遵循 SSE 规范的子集：
//! - 事件以空行分隔（`\n\n`、`\r\n\r\n` 或混用）；
//! - `data:` 多行拼接（`\n` 连接）为一个事件负载；
//! - `event:` 设定事件类型（Anthropic 用），缺省 `message`；
//! - `:` 开头的注释行、`id:` / `retry:` 字段忽略；
//! - 字段值可带一个可选的前导空格（`data: x` 与 `data:x` 等价）。

/// 一个完整的 SSE 事件（空行终止）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// `event:` 字段值；未设置时为 `None`（调用方按协议缺省值处理）。
    pub event: Option<String>,
    /// `data:` 负载（多行以 `\n` 连接）。
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一段字节，返回其中已完整终止的事件。
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut events = Vec::new();
        // 逐个扫描完整事件：找空行（\n\n / \r\n\r\n）作为终止符。
        while let Some(end) = find_event_end(&self.buf) {
            let raw = &self.buf[..end.0];
            events.push(parse_event(raw));
            // 消费掉事件 + 终止符（\r\n\r\n 需多消费 2 字节）。
            self.buf.drain(..end.0 + end.1);
        }
        events
    }

    /// 流结束时的残留（规范上流应以终止空行结束；宽容处理未终止的尾事件）。
    pub fn finish(&mut self) -> Vec<SseEvent> {
        if self.buf.iter().any(|b| !b.is_ascii_whitespace()) {
            let ev = parse_event(&self.buf);
            self.buf.clear();
            vec![ev]
        } else {
            self.buf.clear();
            Vec::new()
        }
    }
}

/// 找第一个事件终止空行：返回 (事件内容结束偏移, 终止符字节数)。
fn find_event_end(buf: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < buf.len() {
        if buf[i] == b'\n' {
            // "\n\n" 或 "\n\r\n"
            if buf.get(i + 1) == Some(&b'\n') {
                return Some((i, 2));
            }
            if buf.get(i + 1) == Some(&b'\r') && buf.get(i + 2) == Some(&b'\n') {
                return Some((i, 3));
            }
        }
        i += 1;
    }
    None
}

/// 解析一个事件块（不含终止空行）。
fn parse_event(raw: &[u8]) -> SseEvent {
    let text = String::from_utf8_lossy(raw);
    let mut data_lines: Vec<String> = Vec::new();
    let mut event: Option<String> = None;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue; // 空行 / 注释
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "data" => data_lines.push(value.to_string()),
            "event" => event = Some(value.to_string()),
            _ => {} // id / retry / 未知字段忽略
        }
    }
    SseEvent {
        event,
        data: data_lines.join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_event_one_chunk() {
        let mut p = SseParser::new();
        let evs = p.feed(b"data: hello\n\n");
        assert_eq!(
            evs,
            vec![SseEvent {
                event: None,
                data: "hello".into()
            }]
        );
    }

    #[test]
    fn splits_across_chunks() {
        let mut p = SseParser::new();
        assert!(p.feed(b"data: he").is_empty());
        assert!(p.feed(b"llo\n").is_empty());
        let evs = p.feed(b"\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "hello");
    }

    #[test]
    fn crlf_terminators_and_value_space() {
        let mut p = SseParser::new();
        let evs = p.feed(b"data: a\r\nevent: delta\r\n\r\ndata:b\r\n\r\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].data, "a");
        assert_eq!(evs[0].event.as_deref(), Some("delta"));
        assert_eq!(evs[1].data, "b");
        assert_eq!(evs[1].event, None);
    }

    #[test]
    fn multiline_data_joined_with_newline() {
        let mut p = SseParser::new();
        let evs = p.feed(b"data: line1\ndata: line2\n\n");
        assert_eq!(evs[0].data, "line1\nline2");
    }

    #[test]
    fn comments_and_unknown_fields_ignored() {
        let mut p = SseParser::new();
        let evs =
            p.feed(b": ping\nevent: content_block_delta\nid: 1\nretry: 100\ndata: {\"x\":1}\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event.as_deref(), Some("content_block_delta"));
        assert_eq!(evs[0].data, "{\"x\":1}");
    }

    #[test]
    fn no_space_after_colon() {
        let mut p = SseParser::new();
        let evs = p.feed(b"data:[DONE]\n\n");
        assert_eq!(evs[0].data, "[DONE]");
    }

    #[test]
    fn several_events_single_chunk() {
        let mut p = SseParser::new();
        let evs = p.feed(b"data: 1\n\ndata: 2\n\ndata: 3\n\n");
        assert_eq!(evs.len(), 3);
        assert_eq!(evs[2].data, "3");
    }

    #[test]
    fn finish_flushes_unterminated_tail() {
        let mut p = SseParser::new();
        assert!(p.feed(b"data: tail").is_empty());
        let evs = p.finish();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "tail");
        // 纯空白残留 = 一个空的合法事件（流循环层会跳过空 data）。
        let evs = p.feed(b"\n\n").to_vec();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "");
        assert!(p.finish().is_empty());
    }

    #[test]
    fn binary_safe_lossy_utf8() {
        let mut p = SseParser::new();
        let evs = p.feed(&[0x64, 0x61, 0x74, 0x61, 0x3a, 0x20, 0xff, 0x0a, 0x0a]);
        assert_eq!(evs[0].data, "\u{fffd}");
    }
}
