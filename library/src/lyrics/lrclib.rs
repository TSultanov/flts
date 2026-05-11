use std::time::Duration;

use log::{info, warn};
use regex::Regex;
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

/// Classifier for `retry()`. Returns `true` for errors that may resolve on a retry.
///
/// Notes:
/// - HTTP 404 never reaches the classifier — `fetch` returns `Ok(None)` for it.
/// - Status-based classification reads back the "LRClib HTTP <code>" message produced
///   below; a typed error enum would be heavier without buying anything for one call site.
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

/// Fetch lyrics for a track from LRClib.
///
/// Returns `Ok(None)` when the track is not in the LRClib database (HTTP 404).
/// Returns `Err` for network / parse failures.
///
/// `duration_s` is optional but improves match quality on LRClib; pass `None` if unknown.
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
    if let Some(album) = album {
        if !album.is_empty() {
            query.push(("album_name", album.to_string()));
        }
    }
    if let Some(duration_s) = duration_s {
        query.push(("duration", duration_s.to_string()));
    }

    let resp = client.get(LRCLIB_BASE).query(&query).send().await?;

    // 404 is "track not in DB" — returned as Ok(None) BEFORE the classifier sees anything,
    // so the retry helper never treats it as transient.
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

/// Parse standard LRC tags: `[mm:ss.xx]text` or `[mm:ss.xxx]text`.
/// Multiple time tags on one text line produce multiple `LyricsLine` entries.
/// Lines with no time tag (or non-LRC metadata like `[ar:...]`, `[ti:...]`)
/// are emitted with `time_ms: None`.
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
            let frac = cap
                .get(3)
                .map(|f| f.as_str())
                .unwrap_or("0");
            // Normalize fractional seconds: ".5" → 500 ms, ".05" → 50 ms, ".005" → 5 ms.
            let frac_ms: u32 = match frac.len() {
                1 => frac.parse::<u32>().unwrap_or(0) * 100,
                2 => frac.parse::<u32>().unwrap_or(0) * 10,
                _ => {
                    let digits: String = frac.chars().take(3).collect();
                    digits.parse::<u32>().unwrap_or(0)
                        * 10u32.pow((3 - digits.len() as u32).max(0))
                }
            };
            times.push(mm * 60_000 + ss * 1_000 + frac_ms);
        }

        let text = raw_line[end_of_tags..].trim().to_string();

        if times.is_empty() {
            out.push(LyricsLine { time_ms: None, text });
        } else {
            for t in times {
                out.push(LyricsLine {
                    time_ms: Some(t),
                    text: text.clone(),
                });
            }
        }
    }

    out.sort_by_key(|l| l.time_ms.unwrap_or(u32::MAX));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(lines.iter().any(|l| l.text == "[ar:Artist]" && l.time_ms.is_none()));
        assert!(lines.iter().any(|l| l.text == "Hello" && l.time_ms == Some(1_000)));
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
