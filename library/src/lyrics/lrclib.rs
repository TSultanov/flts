use std::time::Duration;

use log::{info, warn};
use regex_lite::Regex;
use serde::Deserialize;

use crate::{
    lyrics::{Lyrics, LyricsLine},
    retry::{RetryConfig, retry},
};

const LRCLIB_BASE: &str = "https://lrclib.net/api/get";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT: &str = concat!("FLTS/", env!("CARGO_PKG_VERSION"), " (https://lrclib.net)");

const LRCLIB_RETRY: RetryConfig = RetryConfig {
    max_attempts: 3,
    base_delay: Duration::from_millis(400),
    max_delay: Duration::from_secs(4),
    jitter_frac: 0.25,
};

/// True for errors a retry may resolve. 404 never arrives here — `fetch`
/// returns `Ok(None)` — and status classification parses the numeric code back
/// out of the "LRClib HTTP <code>" message this module produces.
fn is_transient(err: &anyhow::Error) -> bool {
    if let Some(re) = err.downcast_ref::<reqwest::Error>() {
        return re.is_timeout() || re.is_connect() || re.is_request();
    }
    let msg = format!("{err}");
    if let Some(rest) = msg.strip_prefix("LRClib HTTP ")
        && let Some(code_str) = rest.split_whitespace().next()
        && let Ok(code) = code_str.parse::<u16>()
    {
        return code == 408 || code == 429 || (500..=599).contains(&code);
    }
    false
}

#[derive(Debug, Deserialize)]
struct LrclibResponse {
    #[serde(rename = "syncedLyrics")]
    synced_lyrics: Option<String>,
    #[serde(rename = "plainLyrics")]
    plain_lyrics: Option<String>,
}

/// Fetch lyrics for a track. `Ok(None)` means LRClib doesn't have the track.
/// `duration_s` is optional but improves match quality.
pub async fn fetch(
    track_id: &str,
    artist: &str,
    title: &str,
    album: Option<&str>,
    duration_s: Option<u32>,
) -> anyhow::Result<Option<Lyrics>> {
    retry(LRCLIB_RETRY, is_transient, "LRClib fetch", || {
        fetch_once(track_id, artist, title, album, duration_s)
    })
    .await
}

/// The env var carries an origin, not a full endpoint; empty is treated as unset.
fn resolve_get_url(env_origin: Option<String>) -> String {
    match env_origin.filter(|s| !s.is_empty()) {
        Some(origin) => format!("{}/api/get", origin.trim_end_matches('/')),
        None => LRCLIB_BASE.to_string(),
    }
}

async fn fetch_once(
    track_id: &str,
    artist: &str,
    title: &str,
    album: Option<&str>,
    duration_s: Option<u32>,
) -> anyhow::Result<Option<Lyrics>> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()?;

    let mut query: Vec<(&str, String)> = vec![
        ("artist_name", artist.to_string()),
        ("track_name", title.to_string()),
    ];
    if let Some(album) = album
        && !album.is_empty()
    {
        query.push(("album_name", album.to_string()));
    }
    if let Some(duration_s) = duration_s {
        query.push(("duration", duration_s.to_string()));
    }

    let resp = client
        .get(resolve_get_url(std::env::var("FLTS_LRCLIB_BASE_URL").ok()))
        .query(&query)
        .send()
        .await?;

    // 404 means "not in DB"; resolve it before the classifier can retry it.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        info!("LRClib: no lyrics for {artist} — {title}");
        return Ok(None);
    }
    if !resp.status().is_success() {
        // Status encoded numerically so `is_transient` can parse it back.
        anyhow::bail!("LRClib HTTP {}", resp.status().as_u16());
    }

    let body: LrclibResponse = resp.json().await?;

    if let Some(synced) = body.synced_lyrics.as_deref().filter(|s| !s.is_empty()) {
        return Ok(Some(Lyrics {
            track_id: track_id.to_string(),
            lines: parse_lrc(synced),
            synced: true,
        }));
    }

    if let Some(plain) = body.plain_lyrics.as_deref().filter(|s| !s.is_empty()) {
        let lines = plain
            .lines()
            .map(|t| LyricsLine {
                time_ms: None,
                text: t.to_string(),
            })
            .collect();
        return Ok(Some(Lyrics {
            track_id: track_id.to_string(),
            lines,
            synced: false,
        }));
    }

    warn!("LRClib returned 200 with neither syncedLyrics nor plainLyrics");
    Ok(None)
}

/// Parse `[mm:ss.xx]text` tags. Several tags on one line yield several
/// `LyricsLine`s; untagged lines and metadata get `time_ms: None`.
fn parse_lrc(raw: &str) -> Vec<LyricsLine> {
    let time_tag = Regex::new(r"\[(\d{1,3}):(\d{1,2})(?:\.(\d{1,3}))?\]").unwrap();
    let mut out = Vec::new();

    for raw_line in raw.lines() {
        let mut times: Vec<u32> = Vec::new();
        let mut end_of_tags = 0usize;

        for cap in time_tag.captures_iter(raw_line) {
            let m = cap.get(0).unwrap();
            // Only consume consecutive tags from the start of the line.
            if m.start() != end_of_tags {
                break;
            }
            end_of_tags = m.end();

            let mm: u32 = cap[1].parse().unwrap_or(0);
            let ss: u32 = cap[2].parse().unwrap_or(0);
            let frac = cap.get(3).map(|f| f.as_str()).unwrap_or("0");
            // Normalize fractional seconds: ".5" → 500 ms, ".05" → 50 ms, ".005" → 5 ms.
            let frac_ms: u32 = match frac.len() {
                1 => frac.parse::<u32>().unwrap_or(0) * 100,
                2 => frac.parse::<u32>().unwrap_or(0) * 10,
                _ => {
                    let digits: String = frac.chars().take(3).collect();
                    digits.parse::<u32>().unwrap_or(0) * 10u32.pow(3 - digits.len() as u32)
                }
            };
            times.push(mm * 60_000 + ss * 1_000 + frac_ms);
        }

        let text = raw_line[end_of_tags..].trim().to_string();

        if times.is_empty() {
            out.push(LyricsLine {
                time_ms: None,
                text,
            });
        } else {
            for t in times {
                out.push(LyricsLine {
                    time_ms: Some(t),
                    text: text.clone(),
                });
            }
        }
    }

    // Give each untimed line the preceding timed line's stamp so a stable sort
    // keeps it with its stanza instead of dumping it at the end. An ordered
    // file has non-decreasing keys, so the sort is a no-op.
    let mut last_time = 0u32;
    let mut keyed: Vec<(u32, LyricsLine)> = out
        .into_iter()
        .map(|line| {
            let key = match line.time_ms {
                Some(t) => {
                    last_time = t;
                    t
                }
                None => last_time,
            };
            (key, line)
        })
        .collect();
    keyed.sort_by_key(|(key, _)| *key);
    keyed.into_iter().map(|(_, line)| line).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_url_env_resolution() {
        assert_eq!(resolve_get_url(None), LRCLIB_BASE);
        assert_eq!(
            resolve_get_url(Some("http://127.0.0.1:4002/".into())),
            "http://127.0.0.1:4002/api/get"
        );
        assert_eq!(resolve_get_url(Some(String::new())), LRCLIB_BASE);
    }

    #[test]
    fn parse_lrc_basic_timestamps() {
        let raw = "[00:12.34]Hello world\n[01:02.50]Second line";
        let lines = parse_lrc(raw);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].time_ms, Some(12_340));
        assert_eq!(lines[0].text, "Hello world");
        assert_eq!(lines[1].time_ms, Some(62_500));
        assert_eq!(lines[1].text, "Second line");
    }

    #[test]
    fn parse_lrc_three_digit_fractional() {
        let lines = parse_lrc("[00:01.234]Line");
        assert_eq!(lines[0].time_ms, Some(1_234));
    }

    #[test]
    fn parse_lrc_one_digit_fractional() {
        let lines = parse_lrc("[00:01.2]Line");
        assert_eq!(lines[0].time_ms, Some(1_200));
    }

    #[test]
    fn parse_lrc_no_fractional() {
        let lines = parse_lrc("[00:05]Line");
        assert_eq!(lines[0].time_ms, Some(5_000));
    }

    #[test]
    fn parse_lrc_multi_tag_repeats_line() {
        let lines = parse_lrc("[00:01.00][00:10.00]Chorus");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].time_ms, Some(1_000));
        assert_eq!(lines[1].time_ms, Some(10_000));
        assert_eq!(lines[0].text, "Chorus");
        assert_eq!(lines[1].text, "Chorus");
    }

    #[test]
    fn parse_lrc_metadata_keeps_line_without_time() {
        let lines = parse_lrc("[ar:Artist]\n[00:01.00]Hello");
        assert_eq!(lines.len(), 2);
        assert!(
            lines
                .iter()
                .any(|l| l.text == "[ar:Artist]" && l.time_ms.is_none())
        );
        assert!(
            lines
                .iter()
                .any(|l| l.text == "Hello" && l.time_ms == Some(1_000))
        );
    }

    #[test]
    fn is_transient_classifies_status_codes() {
        let t = |code: u16| super::is_transient(&anyhow::anyhow!("LRClib HTTP {code}"));
        assert!(t(408));
        assert!(t(429));
        assert!(t(500));
        assert!(t(502));
        assert!(t(503));
        assert!(t(504));
        assert!(!t(400));
        assert!(!t(401));
        assert!(!t(403));
        assert!(!t(418));
    }

    #[test]
    fn is_transient_ignores_unrelated_errors() {
        assert!(!super::is_transient(&anyhow::anyhow!("some other thing")));
        assert!(!super::is_transient(&anyhow::anyhow!("LRClib HTTP nope")));
    }

    #[test]
    fn parse_lrc_empty_line_text_preserved() {
        let lines = parse_lrc("[00:30.00]");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].time_ms, Some(30_000));
        assert_eq!(lines[0].text, "");
    }
}
