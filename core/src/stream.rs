//! Live-writing streams (word-mcp stream_start/block/end pattern).
//! A stream accumulates markdown blocks with instant preview events and
//! commits them to the handle on end(), returning block/char counts.
//! Caller owns the Stream; relay owns the events.

use crate::bus::Relay;
use crate::ops::{Call, StructArgs, execute};
use crate::protocol::{Result, Error};

#[derive(Debug, Default)]
pub struct Stream {
    pub title: String,
    pub blocks: Vec<String>,
    pub chars: usize,
}

pub fn stream_start(relay: &mut Relay, session: &str, handle: &str, title: &str) -> Result<Stream> {
    relay.emit(session, "stream.start", handle, title.to_string())?;
    Ok(Stream { title: title.into(), blocks: Vec::new(), chars: 0 })
}

pub fn stream_block(relay: &mut Relay, session: &str, handle: &str, stream: &mut Stream, markdown: &str) -> Result<usize> {
    if markdown.len() > 20_000 {
        return Err(Error::OverBulkCap);
    }
    stream.blocks.push(markdown.to_string());
    stream.chars += markdown.len();
    relay.emit(session, "stream.block", handle, format!("block={} chars={}", stream.blocks.len(), stream.chars))?;
    Ok(stream.blocks.len())
}

pub fn stream_end(relay: &mut Relay, session: &str, handle: &str, stream: Stream) -> Result<(usize, usize)> {
    for block in &stream.blocks {
        execute(relay, session, handle, Call::Struct(StructArgs::InsertParagraph { text: block.clone(), style: String::new(), at: String::new() }))?;
    }
    let out = (stream.blocks.len(), stream.chars);
    relay.emit(session, "stream.end", handle, format!("blocks={} chars={}", out.0, out.1))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{FileContent, FileKind, OpenFile};
    use std::collections::HashMap;

    fn word_relay() -> (Relay, String, String) {
        let mut r = Relay::new();
        let s = "s".to_string();
        r.handshake(&s, "t");
        let h = crate::protocol::new_handle("word", "d.docx", "body");
        r.attach(&s, h.clone(), OpenFile {
            kind: FileKind::Word,
            content: FileContent::Word { paras: vec![], tables: vec![], changes: vec![], comments: vec![] },
            styles: HashMap::new(),
        });
        (r, s, h)
    }

    #[test]
    fn stream_commits_with_counts() {
        let (mut r, s, h) = word_relay();
        let mut st = stream_start(&mut r, &s, &h, "Report").unwrap();
        stream_block(&mut r, &s, &h, &mut st, "# Hi").unwrap();
        stream_block(&mut r, &s, &h, &mut st, "body text").unwrap();
        let (blocks, chars) = stream_end(&mut r, &s, &h, st).unwrap();
        assert_eq!((blocks, chars), (2, 13));
        let kinds: Vec<String> = r.events(&s).unwrap().iter().map(|e| e.t.clone()).collect();
        assert!(kinds.contains(&"stream.start".to_string()));
        assert!(kinds.contains(&"stream.end".to_string()));
    }

    #[test]
    fn oversize_block_refused() {
        let (mut r, s, h) = word_relay();
        let mut st = stream_start(&mut r, &s, &h, "T").unwrap();
        assert!(stream_block(&mut r, &s, &h, &mut st, &"x".repeat(20_001)).is_err());
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;
    use crate::bus::{FileContent, FileKind, OpenFile};
    use std::collections::HashMap;

    fn word_relay2() -> (Relay, String, String) {
        let mut r = Relay::new();
        let s = "s".to_string();
        r.handshake(&s, "t");
        let h = crate::protocol::new_handle("word", "d.docx", "body");
        r.attach(&s, h.clone(), OpenFile {
            kind: FileKind::Word,
            content: FileContent::Word { paras: vec![], tables: vec![], changes: vec![], comments: vec![] },
            styles: HashMap::new(),
        });
        (r, s, h)
    }

    #[test]
    fn empty_stream_commits_zero() {
        let (mut r, s, h) = word_relay2();
        let st = stream_start(&mut r, &s, &h, "Empty").unwrap();
        assert_eq!(stream_end(&mut r, &s, &h, st).unwrap(), (0, 0));
        let kinds: Vec<String> = r.events(&s).unwrap().iter().map(|e| e.t.clone()).collect();
        assert!(kinds.contains(&"stream.end".to_string()));
    }

    #[test]
    fn block_index_counts_up() {
        let (mut r, s, h) = word_relay2();
        let mut st = stream_start(&mut r, &s, &h, "T").unwrap();
        assert_eq!(stream_block(&mut r, &s, &h, &mut st, "a").unwrap(), 1);
        assert_eq!(stream_block(&mut r, &s, &h, &mut st, "bc").unwrap(), 2);
        assert_eq!(st.chars, 3);
    }

    #[test]
    fn unknown_session_errors() {
        let (mut r, _, h) = word_relay2();
        assert!(stream_start(&mut r, "ghost", &h, "T").is_err());
    }
}
