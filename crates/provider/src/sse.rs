//! Minimal server-sent-event decoder.
//!
//! `reqwest_eventsource` validates the response `Content-Type` and refuses
//! streams that omit the header. The codex backend (`chatgpt.com` Responses)
//! streams valid SSE payloads with no `Content-Type` at all, so responses
//! adapters decode the body themselves with this module instead.

use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

/// One decoded SSE message: the `event:` field (default `"message"`) and the
/// joined `data:` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseMessage {
    pub event: String,
    pub data: String,
}

/// Wraps a byte stream and yields decoded [`SseMessage`]s. Source errors are
/// forwarded unchanged.
pub(crate) fn decode_sse<S, E>(source: S) -> SseDecoder<S>
where
    S: Stream<Item = Result<Bytes, E>>,
{
    SseDecoder {
        source,
        buffer: Vec::new(),
        event: "message".to_string(),
        data: Vec::new(),
    }
}

pub(crate) struct SseDecoder<S> {
    source: S,
    buffer: Vec<u8>,
    event: String,
    data: Vec<String>,
}

impl<S, E> Stream for SseDecoder<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    type Item = Result<SseMessage, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=newline).collect();
                if let Some(message) = self.handle_line(&line[..line.len() - 1]) {
                    return Poll::Ready(Some(Ok(message)));
                }
                continue;
            }
            match Pin::new(&mut self.source).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => self.buffer.extend_from_slice(&chunk),
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Some(Err(error))),
                Poll::Ready(None) => {
                    if self.buffer.is_empty() {
                        // A server may end the stream without the final blank
                        // line; flush any pending event before finishing.
                        if let Some(message) = self.take_event() {
                            return Poll::Ready(Some(Ok(message)));
                        }
                        return Poll::Ready(None);
                    }
                    let line = std::mem::take(&mut self.buffer);
                    if let Some(message) = self.handle_line(&line) {
                        return Poll::Ready(Some(Ok(message)));
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<S> SseDecoder<S> {
    /// Processes one newline-terminated line; returns a message when the line
    /// completes an event.
    fn handle_line(&mut self, line: &[u8]) -> Option<SseMessage> {
        let line = if line.ends_with(b"\r") {
            &line[..line.len() - 1]
        } else {
            line
        };
        if line.is_empty() {
            return self.take_event();
        }
        if line.starts_with(b":") {
            // Comment lines keep the connection alive; they carry no data.
            return None;
        }
        let (field, value) = match line.iter().position(|byte| *byte == b':') {
            Some(colon) => (&line[..colon], &line[colon + 1..]),
            None => (line, &b""[..]),
        };
        let value = if value.starts_with(b" ") {
            &value[1..]
        } else {
            value
        };
        match field {
            b"event" => self.event = String::from_utf8_lossy(value).into_owned(),
            b"data" => self.data.push(String::from_utf8_lossy(value).into_owned()),
            _ => {}
        }
        None
    }

    fn take_event(&mut self) -> Option<SseMessage> {
        if self.data.is_empty() {
            self.event = "message".to_string();
            return None;
        }
        let message = SseMessage {
            event: std::mem::take(&mut self.event),
            data: self.data.join("\n"),
        };
        self.data.clear();
        self.event = "message".to_string();
        Some(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use pretty_assertions::assert_eq;

    fn decode(chunks: Vec<&[u8]>) -> Vec<SseMessage> {
        let source = stream::iter(
            chunks
                .into_iter()
                .map(|chunk| Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(chunk))),
        );
        futures::executor::block_on_stream(decode_sse(source))
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn decodes_named_events_and_data() {
        let messages = decode(vec![
            b"event: response.created\ndata: {\"id\":1}\n\ndata: [DONE]\n\n".as_slice(),
        ]);
        assert_eq!(
            messages,
            vec![
                SseMessage {
                    event: "response.created".to_string(),
                    data: "{\"id\":1}".to_string(),
                },
                SseMessage {
                    event: "message".to_string(),
                    data: "[DONE]".to_string(),
                },
            ]
        );
    }

    #[test]
    fn decodes_across_chunk_boundaries_and_multiline_data() {
        let messages = decode(vec![
            b"event: response.output_te".as_slice(),
            b"xt.delta\ndata: hel".as_slice(),
            b"lo\ndata: world\n\n".as_slice(),
        ]);
        assert_eq!(
            messages,
            vec![SseMessage {
                event: "response.output_text.delta".to_string(),
                data: "hello\nworld".to_string(),
            }]
        );
    }

    #[test]
    fn skips_comments_and_crlf_and_empty_data_events() {
        let messages = decode(vec![
            b": keep-alive\r\n\r\nevent: ping\r\n\r\nevent: response.done\r\ndata: x\r\n"
                .as_slice(),
        ]);
        assert_eq!(
            messages,
            vec![SseMessage {
                event: "response.done".to_string(),
                data: "x".to_string(),
            }]
        );
    }

    #[test]
    fn flushes_trailing_event_without_final_blank_line() {
        let messages = decode(vec![b"data: {\"done\":true}".as_slice()]);
        assert_eq!(
            messages,
            vec![SseMessage {
                event: "message".to_string(),
                data: "{\"done\":true}".to_string(),
            }]
        );
    }

    #[test]
    fn empty_data_events_are_dropped_not_dispatched() {
        let messages = decode(vec![b"event: ping\n\n\n".as_slice()]);
        assert_eq!(messages, vec![]);
    }
}
