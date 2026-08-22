use unicode_normalization::UnicodeNormalization;

/// Query we are matching LRClib records against.
#[derive(Debug, Clone)]
pub struct LyricsQuery<'a> {
    pub artist: &'a str,
    pub title: &'a str,
    pub album: Option<&'a str>,
    pub duration_s: Option<u32>,
}

/// One LRClib search/get record, before conversion to [`crate::lyrics::Lyrics`].
#[derive(Debug, Clone)]
pub struct LrclibRecord {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub duration: Option<f64>,
    pub instrumental: bool,
    pub synced: Option<String>,
    pub plain: Option<String>,
}

impl LrclibRecord {
    pub fn has_synced(&self) -> bool {
        nonempty(self.synced.as_deref())
    }

    pub fn has_plain(&self) -> bool {
        nonempty(self.plain.as_deref())
    }

    pub fn has_lyrics(&self) -> bool {
        self.has_synced() || self.has_plain()
    }
}

const MAX_DURATION_DELTA_S: f64 = 8.0;

/// Highest-scoring eligible candidate, or `None` if every record is dropped.
/// Ties keep the first remaining candidate.
pub fn pick_best<'a>(
    candidates: &'a [LrclibRecord],
    query: &LyricsQuery<'_>,
) -> Option<&'a LrclibRecord> {
    let mut best: Option<(i32, &'a LrclibRecord)> = None;
    for c in candidates {
        let Some(s) = score(c, query) else { continue };
        match best {
            None => best = Some((s, c)),
            Some((best_s, _)) if s > best_s => best = Some((s, c)),
            _ => {}
        }
    }
    best.map(|(_, c)| c)
}

fn score(candidate: &LrclibRecord, query: &LyricsQuery<'_>) -> Option<i32> {
    if candidate.instrumental && !candidate.has_lyrics() {
        return None;
    }
    if !candidate.has_lyrics() {
        return None;
    }

    let duration_delta = match (candidate.duration, query.duration_s) {
        (Some(cand), Some(q)) => {
            let delta = (cand - f64::from(q)).abs();
            if delta > MAX_DURATION_DELTA_S {
                return None;
            }
            Some(delta)
        }
        _ => None,
    };

    let mut s = if candidate.has_synced() { 1000 } else { 100 };
    if matches_text(candidate.album.as_deref(), query.album) {
        s += 50;
    }
    if matches_text(Some(candidate.artist.as_str()), Some(query.artist)) {
        s += 30;
    }
    if matches_text(Some(candidate.title.as_str()), Some(query.title)) {
        s += 30;
    }
    if let Some(delta) = duration_delta {
        s -= 10 * delta.round() as i32;
    }
    Some(s)
}

fn nonempty(s: Option<&str>) -> bool {
    s.is_some_and(|t| !t.is_empty())
}

fn matches_text(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => normalize(a) == normalize(b),
        _ => false,
    }
}

fn normalize(s: &str) -> String {
    s.nfc().collect::<String>().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query() -> LyricsQuery<'static> {
        LyricsQuery {
            artist: "Mecano",
            title: "Hijo de la Luna",
            album: Some("Entre el Cielo y el Suelo"),
            duration_s: Some(210),
        }
    }

    fn rec() -> LrclibRecord {
        LrclibRecord {
            artist: "Mecano".into(),
            title: "Hijo de la Luna".into(),
            album: Some("Entre el Cielo y el Suelo".into()),
            duration: Some(210.0),
            instrumental: false,
            synced: None,
            plain: None,
        }
    }

    #[test]
    fn synced_beats_plain() {
        let plain = LrclibRecord {
            plain: Some("plain text".into()),
            ..rec()
        };
        let synced = LrclibRecord {
            synced: Some("[00:01.00] timed".into()),
            ..rec()
        };
        let candidates = [plain, synced];
        let best = pick_best(&candidates, &query()).expect("eligible");
        assert_eq!(best.synced.as_deref(), Some("[00:01.00] timed"));
    }

    #[test]
    fn duration_window_rejects_30s_off_synced() {
        let far = LrclibRecord {
            duration: Some(240.0),
            synced: Some("[00:01.00] far".into()),
            ..rec()
        };
        assert!(pick_best(&[far], &query()).is_none());
    }

    #[test]
    fn closer_duration_wins_among_synced() {
        let close = LrclibRecord {
            duration: Some(210.0),
            synced: Some("[00:01.00] close".into()),
            ..rec()
        };
        let farther = LrclibRecord {
            duration: Some(214.0),
            synced: Some("[00:01.00] farther".into()),
            ..rec()
        };
        let candidates = [farther, close];
        let best = pick_best(&candidates, &query()).expect("eligible");
        assert_eq!(best.synced.as_deref(), Some("[00:01.00] close"));
    }

    #[test]
    fn empty_instrumental_is_dropped() {
        let inst = LrclibRecord {
            instrumental: true,
            ..rec()
        };
        assert!(pick_best(&[inst], &query()).is_none());
    }

    #[test]
    fn search_synced_preferred_over_get_plain() {
        let get_plain = LrclibRecord {
            plain: Some("from get".into()),
            ..rec()
        };
        let search_synced = LrclibRecord {
            album: Some("Greatest Hits".into()),
            synced: Some("[00:01.00] from search".into()),
            ..rec()
        };
        let candidates = [get_plain, search_synced];
        let best = pick_best(&candidates, &query()).expect("eligible");
        assert_eq!(best.synced.as_deref(), Some("[00:01.00] from search"));
    }
}
