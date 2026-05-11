use std::path::{Path, PathBuf};

use isolang::Language;
use log::{info, warn};
use tokio::{fs, io::AsyncWriteExt};

use crate::lyrics::LyricsTranslation;

const CACHE_SUBDIR: &str = "lyrics";

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
        info!("LyricsCache: wrote {} ({} bytes)", path.display(), bytes.len());
        Ok(())
    }

    fn path_for(&self, track_id: &str, target: &Language, model: usize) -> PathBuf {
        let safe_track = sanitize(track_id);
        let filename = format!(
            "{safe_track}__{}_{}.json",
            target.to_639_3(),
            model
        );
        self.root.join(filename)
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
    use crate::lyrics::{Gloss, LyricsLineTranslation};
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
}
