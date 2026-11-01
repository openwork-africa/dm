//! How a stream's bytes are cut into frames: server-sent events, or AWS's binary event stream (Bedrock).

use anyhow::{bail, Result};

/// One frame: its event name, when the stream gives one, and its data.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub name: Option<String>,
    pub data: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    Sse,
    AwsEventStream,
}

pub trait Decoder {
    /// The frames these bytes complete.
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>>;
    /// What is left once the stream ends.
    fn finish(&mut self) -> Result<Vec<Frame>>;
}

impl Framing {
    pub fn decoder(self) -> Box<dyn Decoder> {
        match self {
            Framing::Sse => Box::<Sse>::default(),
            Framing::AwsEventStream => Box::<AwsEventStream>::default(),
        }
    }
}

#[derive(Default)]
struct Sse {
    buffer: Vec<u8>,
    name: Option<String>,
    data: Vec<String>,
}

impl Sse {
    fn line(&mut self, line: &str, frames: &mut Vec<Frame>) {
        if line.is_empty() {
            self.flush(frames);
        } else if let Some(name) = line.strip_prefix("event:") {
            self.name = Some(name.trim().to_string());
        } else if let Some(data) = line.strip_prefix("data:") {
            self.data
                .push(data.strip_prefix(' ').unwrap_or(data).to_string());
        }
    }

    fn flush(&mut self, frames: &mut Vec<Frame>) {
        if !self.data.is_empty() {
            frames.push(Frame {
                name: self.name.take(),
                data: self.data.join("\n"),
            });
        }
        self.name = None;
        self.data.clear();
    }
}

impl Decoder for Sse {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = vec![];
        while let Some(end) = self.buffer.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = self.buffer.drain(..=end).collect();
            let line = String::from_utf8_lossy(&raw);
            self.line(line.trim_end_matches(['\n', '\r']), &mut frames);
        }
        Ok(frames)
    }

    fn finish(&mut self) -> Result<Vec<Frame>> {
        let mut frames = vec![];
        let rest = String::from_utf8_lossy(&std::mem::take(&mut self.buffer)).to_string();
        self.line(rest.trim_end_matches('\r'), &mut frames);
        self.flush(&mut frames);
        Ok(frames)
    }
}

/// AWS event stream messages: a 12-byte prelude (total length, headers length, prelude CRC), headers, the payload, and
/// a message CRC. The CRCs are not checked; the stream already comes over TLS. An exception message becomes a frame
/// named `exception:<type>`.
#[derive(Default)]
struct AwsEventStream {
    buffer: Vec<u8>,
}

const PRELUDE: usize = 12;

fn be_u32(bytes: &[u8]) -> usize {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
}

/// The string headers (`:event-type`, `:message-type`, `:exception-type`); every other kind is skipped.
fn string_headers(mut bytes: &[u8]) -> Result<Vec<(String, String)>> {
    let mut headers = vec![];
    while !bytes.is_empty() {
        let name_len = bytes[0] as usize;
        let name =
            String::from_utf8_lossy(bytes.get(1..1 + name_len).unwrap_or_default()).to_string();
        let Some(&kind) = bytes.get(1 + name_len) else {
            bail!("a cut-off event stream header")
        };
        let rest = &bytes[2 + name_len..];
        let fixed = |n: usize| -> Result<usize> {
            if rest.len() < n {
                bail!("a cut-off event stream header")
            }
            Ok(n)
        };
        let used = match kind {
            0 | 1 => 0,
            2 => fixed(1)?,
            3 => fixed(2)?,
            4 => fixed(4)?,
            5 | 8 => fixed(8)?,
            9 => fixed(16)?,
            6 | 7 => {
                fixed(2)?;
                let len = u16::from_be_bytes([rest[0], rest[1]]) as usize;
                let total = fixed(2 + len)?;
                if kind == 7 {
                    headers.push((name, String::from_utf8_lossy(&rest[2..total]).to_string()));
                }
                total
            }
            other => bail!("an event stream header of unknown type {other}"),
        };
        bytes = &rest[used..];
    }
    Ok(headers)
}

impl Decoder for AwsEventStream {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = vec![];
        while self.buffer.len() >= PRELUDE {
            let total = be_u32(&self.buffer[0..4]);
            let headers_len = be_u32(&self.buffer[4..8]);
            if total < PRELUDE + headers_len + 4 {
                bail!("a malformed event stream message");
            }
            if self.buffer.len() < total {
                break;
            }
            let message: Vec<u8> = self.buffer.drain(..total).collect();
            let headers = string_headers(&message[PRELUDE..PRELUDE + headers_len])?;
            let header = |key: &str| {
                headers
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
            };
            let data =
                String::from_utf8_lossy(&message[PRELUDE + headers_len..total - 4]).to_string();
            let name = match header(":message-type").as_deref() {
                Some("exception") | Some("error") => Some(format!(
                    "exception:{}",
                    header(":exception-type")
                        .or(header(":error-code"))
                        .unwrap_or_default()
                )),
                _ => header(":event-type"),
            };
            frames.push(Frame { name, data });
        }
        Ok(frames)
    }

    fn finish(&mut self) -> Result<Vec<Frame>> {
        if !self.buffer.is_empty() {
            bail!("the event stream ended part way through a message");
        }
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_frames_survive_any_split() {
        let stream = b"event: a\ndata: {\"x\":1}\n\ndata: two\r\ndata: lines\r\n\r\ndata: [DONE]";
        for cut in 0..stream.len() {
            let mut decoder = Framing::Sse.decoder();
            let mut frames = decoder.push(&stream[..cut]).unwrap();
            frames.extend(decoder.push(&stream[cut..]).unwrap());
            frames.extend(decoder.finish().unwrap());
            assert_eq!(
                frames,
                vec![
                    Frame {
                        name: Some("a".into()),
                        data: "{\"x\":1}".into()
                    },
                    Frame {
                        name: None,
                        data: "two\nlines".into()
                    },
                    Frame {
                        name: None,
                        data: "[DONE]".into()
                    },
                ]
            );
        }
    }
}
