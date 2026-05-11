use std::path::{Path, PathBuf};

use isolang::Language;
use log::{info, warn};
use tokio::{fs, io::AsyncWriteExt};

use crate::lyrics::{Lyrics, LyricsTranslation};

const CACHE_SUBDIR: &str = "lyrics";
const RAW_SUBDIR: &str = "raw";

/// Disk cache for translated lyrics, keyed by (track_id, source_lang, target_lang, model).
///
/// Stored as one JSON file per entry under `<cache_dir>/lyrics/<key>.json`.
/// Created lazily on first write.
pub struct LyricsCache {
    root: PathBuf,
}

impl LyricsCache {
    pub fn new(cache_dir: &Path) -> Self {
        Self {
            root: cache_dir.join(CACHE_SUBDIR),
        }
    }

    pub async fn get(
        &self,
        track_id: &str,
        target: &Language,
        model: usize,
    ) -> Option<LyricsTranslation> {
        let path = self.path_for(track_id, target, model);
        match fs::read(&path).await {
            Ok(bytes) => match serde_json::from_slice::<LyricsTranslation>(&bytes) {
                Ok(v) => Some(v),
                Err(err) => {
                    warn!(
                        "LyricsCache: corrupt entry at {} ({err}); ignoring",
                        path.display()
                    );
                    None
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                warn!("LyricsCache: read error at {}: {err}", path.display());
                None
            }
        }
    }

    pub async fn put(&self, t: &LyricsTranslation) -> anyhow::Result<()> {
        fs::create_dir_all(&self.root).await?;
        let path = self.path_for(&t.track_id, &t.target_lang, t.model);
        let bytes = serde_json::to_vec(t)?;

        // Write to a temp file in the same dir, then rename — atomic on POSIX.
        let tmp = path.with_extension("json.tmp");
        let mut f = fs::File::create(&tmp).await?;
        f.write_all(&bytes).await?;
        f.flush().await?;
        drop(f);
        fs::rename(&tmp, &path).await?;
        info!(
            "LyricsCache: wrote {} ({} bytes)",
            path.display(),
            bytes.len()
        );
        Ok(())
    }

    fn path_for(&self, track_id: &str, target: &Language, model: usize) -> PathBuf {
        let safe_track = sanitize(track_id);
        let filename = format!("{safe_track}__{}_{}.json", target.to_639_3(), model);
        self.root.join(filename)
    }

    /// Look up the raw (untranslated) lyrics for a track. Cache miss / corruption / I/O
    /// errors all return `None` — the caller should fall back to a fresh fetch.
    pub async fn get_raw(&self, track_id: &str) -> Option<Lyrics> {
        let path = self.raw_path_for(track_id);
        match fs::read(&path).await {
            Ok(bytes) => match serde_json::from_slice::<Lyrics>(&bytes) {
                Ok(v) => Some(v),
                Err(err) => {
                    warn!(
                        "LyricsCache: corrupt raw entry at {} ({err}); ignoring",
                        path.display()
                    );
                    None
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                warn!("LyricsCache: raw read error at {}: {err}", path.display());
                None
            }
        }
    }

    /// Persist the raw lyrics for a track. Stored under `<root>/raw/<track>.json` so
    /// it can't collide with translation entries that live directly under `<root>/`.
    pub async fn put_raw(&self, lyrics: &Lyrics) -> anyhow::Result<()> {
        let dir = self.root.join(RAW_SUBDIR);
        fs::create_dir_all(&dir).await?;
        let path = self.raw_path_for(&lyrics.track_id);
        let bytes = serde_json::to_vec(lyrics)?;

        let tmp = path.with_extension("json.tmp");
        let mut f = fs::File::create(&tmp).await?;
        f.write_all(&bytes).await?;
        f.flush().await?;
        drop(f);
        fs::rename(&tmp, &path).await?;
        info!(
            "LyricsCache: wrote raw {} ({} bytes)",
            path.display(),
            bytes.len()
        );
        Ok(())
    }

    fn raw_path_for(&self, track_id: &str) -> PathBuf {
        let safe = sanitize(track_id);
        self.root.join(RAW_SUBDIR).join(format!("{safe}.json"))
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::{Gloss, LyricsLine, LyricsLineTranslation};
    use crate::test_utils::TempDir;

    fn sample(track_id: &str) -> LyricsTranslation {
        LyricsTranslation {
            track_id: track_id.to_string(),
            target_lang: Language::from_639_3("eng").unwrap(),
            model: 4,
            lines: vec![LyricsLineTranslation {
                translation: "hello".into(),
                glosses: vec![Gloss {
                    fragment: "Hallo".into(),
                    gloss: "hello".into(),
                    note: "".into(),
                }],
            }],
        }
    }

    #[tokio::test]
    async fn roundtrip() {
        let dir = TempDir::new("flts_lyrics_cache");
        let cache = LyricsCache::new(&dir.path);
        let t = sample("spotify:track:abc123");
        cache.put(&t).await.unwrap();
        let got = cache
            .get("spotify:track:abc123", &t.target_lang, t.model)
            .await
            .expect("cache hit");
        assert_eq!(got.lines.len(), 1);
        assert_eq!(got.lines[0].translation, "hello");
    }

    #[tokio::test]
    async fn miss_returns_none() {
        let dir = TempDir::new("flts_lyrics_cache");
        let cache = LyricsCache::new(&dir.path);
        let got = cache
            .get(
                "spotify:track:nope",
                &Language::from_639_3("eng").unwrap(),
                4,
            )
            .await;
        assert!(got.is_none());
    }

    fn sample_raw(track_id: &str) -> Lyrics {
        Lyrics {
            track_id: track_id.to_string(),
            synced: true,
            lines: vec![
                LyricsLine {
                    time_ms: Some(1_000),
                    text: "Hallo Welt".into(),
                },
                LyricsLine {
                    time_ms: Some(2_500),
                    text: "Wie geht's?".into(),
                },
            ],
        }
    }

    #[tokio::test]
    async fn raw_roundtrip() {
        let dir = TempDir::new("flts_lyrics_cache_raw");
        let cache = LyricsCache::new(&dir.path);
        let raw = sample_raw("spotify:track:abc123");
        cache.put_raw(&raw).await.unwrap();
        let got = cache
            .get_raw("spotify:track:abc123")
            .await
            .expect("raw cache hit");
        assert!(got.synced);
        assert_eq!(got.lines.len(), 2);
        assert_eq!(got.lines[0].text, "Hallo Welt");
        assert_eq!(got.lines[1].time_ms, Some(2_500));
    }

    #[tokio::test]
    async fn raw_miss_returns_none() {
        let dir = TempDir::new("flts_lyrics_cache_raw_miss");
        let cache = LyricsCache::new(&dir.path);
        assert!(cache.get_raw("spotify:track:nope").await.is_none());
    }

    #[tokio::test]
    async fn raw_and_translation_do_not_collide() {
        // Translation files live at <root>/<track>__<lang>_<model>.json,
        // raw files at <root>/raw/<track>.json — same track id must not overwrite either.
        let dir = TempDir::new("flts_lyrics_cache_both");
        let cache = LyricsCache::new(&dir.path);
        let t = sample("spotify:track:abc");
        let r = sample_raw("spotify:track:abc");
        cache.put(&t).await.unwrap();
        cache.put_raw(&r).await.unwrap();

        let got_t = cache
            .get("spotify:track:abc", &t.target_lang, t.model)
            .await
            .expect("translation hit");
        let got_r = cache.get_raw("spotify:track:abc").await.expect("raw hit");
        assert_eq!(got_t.lines[0].translation, "hello");
        assert_eq!(got_r.lines[0].text, "Hallo Welt");
    }
}
