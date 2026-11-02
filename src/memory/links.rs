//! Links `remember` reads: the content an agent is given access to ("Content you have access to" in its instructions),
//! each a short-lived signed link. What a link returns is not handed to the model as it is: it is cut into passages of
//! about PASSAGE characters along paragraph and sentence breaks, indexed in a throwaway in-memory FTS5 table for that one
//! call, ranked by BM25 against the query, and only the HITS best passages come back, each with the link it is from.
//! With no query the first passages come back, in the order they are written.
//!
//! A link is read over HTTPS, or over plain HTTP on this machine only (a workspace's own server); redirects are held to
//! the same rule. It is read for at most TIMEOUT and MAX_BYTES, and only text is read (text/*, markdown, JSON); HTML is
//! read as its text. PDFs and other binary files are refused with a line that says so.

use anyhow::{anyhow, bail, Result};
use reqwest::{redirect, Url};
use rusqlite::{params, Connection};
use std::time::Duration;

/// Passages returned, at most.
pub const HITS: usize = 6;
/// Passages are about this many characters.
pub const PASSAGE: usize = 600;
/// Links read in one call, at most.
pub const MAX_LINKS: usize = 10;
/// How long one link may take, and the most it may weigh.
pub const TIMEOUT: Duration = Duration::from_secs(20);
pub const MAX_BYTES: usize = 5 * 1024 * 1024;
const REDIRECTS: usize = 5;
const NOTHING: &str = "Nothing in that content matches what you are looking for.";

/// The limits one read is held to; tests make them small.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub timeout: Duration,
    pub max_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: TIMEOUT,
            max_bytes: MAX_BYTES,
        }
    }
}

/// One link read: its title and its text.
#[derive(Debug, Clone)]
pub struct Page {
    pub title: String,
    pub url: String,
    pub text: String,
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// HTTPS anywhere; plain HTTP only on this machine.
pub fn allowed(url: &Url) -> bool {
    match url.scheme() {
        "https" => url.host().is_some(),
        "http" => is_loopback(url),
        _ => false,
    }
}

/// A link as it is shown: without its query, which holds the signature.
pub fn shown(url: &Url) -> String {
    let mut bare = url.clone();
    bare.set_query(None);
    bare.set_fragment(None);
    bare.to_string()
}

fn client(limits: Limits) -> Result<reqwest::Client> {
    let policy = redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= REDIRECTS {
            attempt.error("too many redirects")
        } else if allowed(attempt.url()) {
            attempt.follow()
        } else {
            attempt.error("a redirect left HTTPS")
        }
    });
    Ok(reqwest::Client::builder()
        .redirect(policy)
        .timeout(limits.timeout)
        .connect_timeout(limits.timeout)
        .build()?)
}

enum Kind {
    Text,
    Html,
}

fn kind_of(content_type: &str) -> Option<Kind> {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match essence.as_str() {
        "text/html" | "application/xhtml+xml" => Some(Kind::Html),
        "" | "application/json" | "application/xml" | "application/x-ndjson" => Some(Kind::Text),
        other
            if other.starts_with("text/")
                || other.ends_with("+json")
                || other.ends_with("+xml") =>
        {
            Some(Kind::Text)
        }
        _ => None,
    }
}

/// The file name a response gives itself (content-disposition), if any.
fn file_name(response: &reqwest::Response) -> Option<String> {
    let value = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)?
        .to_str()
        .ok()?;
    let name = value
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("filename="))?;
    let name = name.trim_matches('"').trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The title of what was read: the name it gives itself, else its first Markdown heading, else the link without its query.
fn title_of(name: Option<String>, url: &Url, text: &str) -> String {
    name.or_else(|| {
        text.lines()
            .find_map(|line| line.strip_prefix("# "))
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty())
    })
    .unwrap_or_else(|| shown(url))
}

/// The words of an HTML page: its scripts and styles left out, its tags taken away, blocks kept as paragraphs.
pub fn html_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::new();
    let mut at = 0;
    while at < html.len() {
        let Some(open) = lower[at..].find('<').map(|i| at + i) else {
            out.push_str(&html[at..]);
            break;
        };
        out.push_str(&html[at..open]);
        let rest = &lower[open..];
        let skip = ["script", "style"].iter().find(|tag| {
            rest[1..].starts_with(*tag)
                && rest[1 + tag.len()..].starts_with(|c: char| c == '>' || c.is_whitespace())
        });
        if let Some(tag) = skip {
            let close = format!("</{tag}");
            at = rest
                .find(&close)
                .and_then(|i| lower[open + i..].find('>').map(|j| open + i + j + 1))
                .unwrap_or(html.len());
            continue;
        }
        let end = lower[open..].find('>').map_or(html.len(), |i| open + i + 1);
        let name = rest[1..]
            .trim_start_matches('/')
            .split(|c: char| !c.is_ascii_alphanumeric())
            .next()
            .unwrap_or_default();
        if matches!(
            name,
            "p" | "div"
                | "br"
                | "li"
                | "tr"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "section"
                | "article"
        ) {
            out.push_str("\n\n");
        }
        at = end;
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Reads one link within the limits.
pub async fn read(raw: &str, limits: Limits) -> Result<Page> {
    let url = Url::parse(raw.trim()).map_err(|_| anyhow!("that is not a link"))?;
    if !allowed(&url) {
        bail!("only https links can be read");
    }
    let shown_url = shown(&url);
    let mut response = client(limits)?
        .get(url.clone())
        .send()
        .await
        .map_err(|err| {
            if err.is_timeout() {
                anyhow!("it took longer than {} seconds", limits.timeout.as_secs())
            } else {
                anyhow!("it could not be reached")
            }
        })?;
    let status = response.status();
    if !status.is_success() {
        bail!(
            "it answered {}{}",
            status.as_u16(),
            match status.as_u16() {
                401 | 403 | 404 | 410 => " (the link may have expired; links last 30 minutes)",
                _ => "",
            }
        );
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let Some(kind) = kind_of(&content_type) else {
        bail!("it is {content_type}, and only text can be read");
    };
    if response
        .content_length()
        .is_some_and(|n| n as usize > limits.max_bytes)
    {
        bail!("it is larger than {} KiB", limits.max_bytes / 1024);
    }
    let name = file_name(&response);
    let mut body = vec![];
    loop {
        let chunk = tokio::time::timeout(limits.timeout, response.chunk())
            .await
            .map_err(|_| anyhow!("it took longer than {} seconds", limits.timeout.as_secs()))?
            .map_err(|err| {
                if err.is_timeout() {
                    anyhow!("it took longer than {} seconds", limits.timeout.as_secs())
                } else {
                    anyhow!("it could not be read whole")
                }
            })?;
        let Some(chunk) = chunk else { break };
        if body.len() + chunk.len() > limits.max_bytes {
            bail!("it is larger than {} KiB", limits.max_bytes / 1024);
        }
        body.extend_from_slice(&chunk);
    }
    let text = String::from_utf8_lossy(&body).into_owned();
    let text = match kind {
        Kind::Html => html_text(&text),
        Kind::Text => text,
    };
    Ok(Page {
        title: title_of(name, &url, &text),
        url: shown_url,
        text,
    })
}

/// Text cut into passages of about PASSAGE characters, along paragraph and sentence breaks.
pub fn passages(text: &str) -> Vec<String> {
    let mut pieces: Vec<&str> = vec![];
    let mut last = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let end = if bytes[i] == b'\n' && bytes.get(i + 1) == Some(&b'\n') {
            let mut j = i;
            while bytes.get(j) == Some(&b'\n') {
                j += 1;
            }
            Some(j)
        } else if matches!(bytes[i], b'.' | b'!' | b'?')
            && bytes.get(i + 1).is_some_and(|b| b.is_ascii_whitespace())
        {
            let mut j = i + 1;
            while bytes.get(j).is_some_and(|b| b.is_ascii_whitespace()) {
                j += 1;
            }
            Some(j)
        } else {
            None
        };
        match end {
            Some(j) => {
                pieces.push(&text[last..j]);
                last = j;
                i = j;
            }
            None => i += 1,
        }
    }
    pieces.push(&text[last..]);
    let mut parts = vec![];
    let mut current = String::new();
    for piece in pieces {
        if !current.is_empty() && current.len() + piece.len() > PASSAGE {
            parts.push(current.trim().to_string());
            current.clear();
        }
        current.push_str(piece);
        // A piece with no break in it is cut where it runs past twice the size, at a character's edge.
        while current.len() > 2 * PASSAGE {
            let mut cut = PASSAGE;
            while !current.is_char_boundary(cut) {
                cut += 1;
            }
            let rest = current.split_off(cut);
            parts.push(current.trim().to_string());
            current = rest;
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts.retain(|p| !p.is_empty());
    parts
}

/// Each word quoted, so nothing in it is read as an operator; any of them may match.
pub fn fts_query(query: &str) -> Option<String> {
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\""))
        .collect();
    (!words.is_empty()).then(|| words.join(" OR "))
}

/// What was read, narrowed with a throwaway FTS5 index to the HITS passages that best match the query (BM25), each
/// with the link it is from. With no query, the first HITS passages.
pub fn focus(query: &str, pages: &[Page]) -> Result<String> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(
        "CREATE VIRTUAL TABLE passages USING fts5 (title UNINDEXED, url UNINDEXED, text, tokenize = 'porter unicode61');",
    )?;
    for page in pages {
        for text in passages(&page.text) {
            conn.execute(
                "INSERT INTO passages (title, url, text) VALUES (?1, ?2, ?3)",
                params![page.title, page.url, text],
            )?;
        }
    }
    let row = |r: &rusqlite::Row| -> rusqlite::Result<(String, String, String)> {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    };
    let hits: Vec<(String, String, String)> = match fts_query(query) {
        Some(fts) => conn
            .prepare("SELECT title, url, text FROM passages WHERE passages MATCH ?1 ORDER BY bm25(passages) LIMIT ?2")?
            .query_map(params![fts, HITS], row)?
            .collect::<rusqlite::Result<_>>()?,
        None => conn
            .prepare("SELECT title, url, text FROM passages ORDER BY rowid LIMIT ?1")?
            .query_map(params![HITS], row)?
            .collect::<rusqlite::Result<_>>()?,
    };
    Ok(if hits.is_empty() {
        NOTHING.to_string()
    } else {
        hits.iter()
            .map(|(title, url, text)| format!("{title} ({url}):\n{text}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    })
}

/// The links given, as one or several: a string, or an array of strings.
pub fn links_in(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(one) => vec![one.clone()],
        serde_json::Value::Array(many) => many
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => vec![],
    }
    .into_iter()
    .map(|l| l.trim().to_string())
    .filter(|l| !l.is_empty())
    .collect()
}

/// Reads each link (at most MAX_LINKS) and answers with the passages that best match the query; a link that could not
/// be read is named, without its signature, with why.
pub async fn recall(links: &[String], query: &str, limits: Limits) -> Result<String> {
    if links.is_empty() {
        bail!("there is no link to read");
    }
    let mut pages = vec![];
    let mut failed = vec![];
    for link in links.iter().take(MAX_LINKS) {
        match read(link, limits).await {
            Ok(page) => pages.push(page),
            Err(err) => {
                let named = Url::parse(link).map_or_else(|_| "A link".to_string(), |u| shown(&u));
                failed.push(format!("{named} could not be read: {err:#}."));
            }
        }
    }
    if links.len() > MAX_LINKS {
        failed.push(format!("Only the first {MAX_LINKS} links were read."));
    }
    let mut parts = vec![];
    if !pages.is_empty() {
        parts.push(focus(query, &pages)?);
    }
    parts.extend(failed);
    Ok(parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_this_machine() {
        let ok = |u: &str| allowed(&Url::parse(u).unwrap());
        assert!(ok("https://worklyn.me/content/x"));
        assert!(ok("http://127.0.0.1:8080/x"));
        assert!(ok("http://localhost/x"));
        assert!(ok("http://[::1]/x"));
        assert!(!ok("http://worklyn.me/x"));
        assert!(!ok("http://10.0.0.1/x"));
        assert!(!ok("file:///etc/passwd"));
        assert!(!ok("ftp://example.com/x"));
    }

    #[test]
    fn a_link_is_shown_without_its_signature() {
        let url = Url::parse("https://worklyn.me/content/abc?sig=secret#x").unwrap();
        assert_eq!(shown(&url), "https://worklyn.me/content/abc");
    }

    #[test]
    fn passages_follow_breaks_and_stay_near_the_size() {
        let sentence = "The quick brown fox jumps over the lazy dog. ";
        let text = format!("{}\n\n{}", sentence.repeat(30), sentence.repeat(30));
        let parts = passages(&text);
        assert!(parts.len() >= 4);
        assert!(parts.iter().all(|p| p.len() <= PASSAGE + sentence.len()));
        assert!(parts.iter().all(|p| p.ends_with('.')));
        let long = "x".repeat(5000);
        assert!(passages(&long).iter().all(|p| p.len() <= 2 * PASSAGE));
    }

    #[test]
    fn at_most_six_passages_ranked_by_the_query() {
        let mut text = String::new();
        for n in 0..40 {
            text.push_str(&format!(
                "Paragraph {n} talks about llamas and the weather.\n\n"
            ));
            text.push_str(&"Filler words fill the space here. ".repeat(20));
            text.push_str("\n\n");
        }
        text.push_str("The secret recipe needs saffron.\n\n");
        let page = Page {
            title: "Notes".into(),
            url: "https://x/content/1".into(),
            text,
        };
        let found = focus("saffron recipe", std::slice::from_ref(&page)).unwrap();
        let best = found.split("\n\nNotes (").next().unwrap();
        assert!(best.starts_with("Notes (https://x/content/1):\n") && best.contains("saffron"));
        let many = focus("llamas", std::slice::from_ref(&page)).unwrap();
        assert_eq!(many.matches("(https://x/content/1):").count(), HITS);
        let first = focus("", &[page]).unwrap();
        assert!(first.starts_with("Notes (https://x/content/1):\nParagraph 0"));
        assert_eq!(first.matches("(https://x/content/1):").count(), HITS);
    }

    #[test]
    fn html_is_read_as_its_words() {
        let text = html_text("<html><head><style>p{}</style><script>alert(1)</script></head><body><h1>Title</h1><p>One &amp; two.</p></body></html>");
        assert!(!text.contains("alert") && !text.contains("p{}"));
        assert!(text.contains("Title") && text.contains("One & two."));
    }

    #[test]
    fn links_are_one_or_several() {
        assert_eq!(links_in(&serde_json::json!("https://a")), ["https://a"]);
        assert_eq!(
            links_in(&serde_json::json!(["https://a", " ", "https://b"])),
            ["https://a", "https://b"]
        );
        assert!(links_in(&serde_json::json!(null)).is_empty());
    }
}
