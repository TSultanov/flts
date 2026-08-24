use serde::Deserialize;

use crate::lyrics::{Lyrics, LyricsLine};

/// Parses the `color-lyrics/v2/track/{id}` payload (same shape as the legacy
/// `/lyrics/v1/track`). The app layer obtains this JSON by evaluating the
/// request inside Spotify's own desktop webview (see
/// `app/src-tauri/app/spotify/cdp.rs`) — there is deliberately no direct HTTP
/// path here: Spotify's lyrics endpoints only accept the desktop session's
/// internal token, and third-party use of that surface risks account bans.
///
/// `syncType` is uppercase (`LINE_SYNCED`/`WORD_SYNCED`/`UNSYNCED`),
/// `startTimeMs` is a string that may be empty for unsynced lines.
pub fn parse_lyrics(track_id: &str, payload: LyricsResponse) -> Lyrics {
    let synced = payload.sync_type == "LINE_SYNCED" || payload.sync_type == "WORD_SYNCED";
    let lines = payload
        .lines
        .into_iter()
        .map(|l| LyricsLine {
            time_ms: if synced {
                l.start_time_ms().filter(|v| *v > 0)
            } else {
                None
            },
            text: l.words,
        })
        .collect();
    Lyrics {
        track_id: track_id.to_string(),
        lines,
        synced,
    }
}

#[derive(Debug, Deserialize)]
pub struct LyricsResponse {
    #[serde(rename = "syncType")]
    pub sync_type: String,
    pub lines: Vec<LyricsLineResponse>,
}

#[derive(Debug, Deserialize)]
pub struct LyricsLineResponse {
    #[serde(rename = "startTimeMs", default)]
    pub start_time_ms: String,
    pub words: String,
}

impl LyricsLineResponse {
    fn start_time_ms(&self) -> Option<u32> {
        self.start_time_ms.parse::<u32>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_line_synced() {
        let payload: LyricsResponse = serde_json::from_str(
            r#"{"syncType":"LINE_SYNCED","lines":[
                 {"startTimeMs":"960","words":"One, two","endTimeMs":"0","syllables":[]},
                 {"startTimeMs":"4020","words":"Ooh-ooh","endTimeMs":"0","syllables":[]}
               ]}"#,
        )
        .unwrap();
        let l = parse_lyrics("t1", payload);
        assert!(l.synced);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[0].time_ms, Some(960));
        assert_eq!(l.lines[1].time_ms, Some(4020));
        assert_eq!(l.lines[1].text, "Ooh-ooh");
    }

    #[test]
    fn parses_unsynced() {
        let payload: LyricsResponse = serde_json::from_str(
            r#"{"syncType":"UNSYNCED","lines":[
                 {"startTimeMs":"","words":"line one"},
                 {"startTimeMs":"","words":"line two"}
               ]}"#,
        )
        .unwrap();
        let l = parse_lyrics("spot1", payload);
        assert!(!l.synced);
        assert!(l.lines.iter().all(|x| x.time_ms.is_none()));
        assert_eq!(l.lines[0].text, "line one");
    }
}
