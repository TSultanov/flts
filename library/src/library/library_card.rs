use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use tokio::sync::{Mutex, Notify};

use crate::{
    book::serialization::create_random_string,
    card::{Card, card_id, familiarity_from, lemma_slug},
};

pub struct LibraryCardStore {
    root: PathBuf,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    change_notify: Arc<Notify>,
    /// Reader-side familiarity scalar per `card_id(src, tgt, slug)`, holding
    /// the result of [`familiarity_from`] so page renders never re-read or
    /// re-parse card JSON. `None` = dormant (Suspended/Deleted), `Some(0.0)`
    /// = never-synced / no file on disk, `Some(scalar)` = active. Kept fresh
    /// by `save_inner` (our own writes) and `invalidate_familiarity` (changes
    /// that land on disk from outside, e.g. Syncthing). The lock is only ever
    /// held around the map operation itself, never across an `.await`.
    fam_cache: RwLock<HashMap<String, Option<f32>>>,
}

impl LibraryCardStore {
    pub fn new(library_root: &Path) -> Self {
        Self {
            root: library_root.join("cards"),
            locks: Mutex::new(HashMap::new()),
            change_notify: Arc::new(Notify::new()),
            fam_cache: RwLock::new(HashMap::new()),
        }
    }

    /// Returns a handle to the wake signal that fires after every successful
    /// `save` (but not `save_without_wake`). Used by the Anki sync task to
    /// trigger a sync pass as soon as a card lands on disk.
    pub fn change_notify(&self) -> Arc<Notify> {
        self.change_notify.clone()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn deck_dir(&self, source_language: &str, target_language: &str) -> PathBuf {
        self.root
            .join(format!("{source_language}-{target_language}"))
    }

    pub fn card_path(
        &self,
        source_language: &str,
        target_language: &str,
        lemma_slug: &str,
    ) -> PathBuf {
        self.deck_dir(source_language, target_language)
            .join(format!("{lemma_slug}.json"))
    }

    pub async fn lock_for(&self, id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        locks
            .entry(id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Read and parse the canonical card file only — no Syncthing conflict
    /// scan and no writeback, so it touches a single file and never enumerates
    /// the deck directory. This is the cheap read backing the reader-side
    /// familiarity path; use [`load`] when conflict reconciliation is required
    /// (the write/sync paths).
    pub async fn load_canonical(
        &self,
        source_language: &str,
        target_language: &str,
        lemma_slug: &str,
    ) -> anyhow::Result<Option<Card>> {
        let canonical_path = self.card_path(source_language, target_language, lemma_slug);
        if !tokio::fs::try_exists(&canonical_path).await? {
            return Ok(None);
        }
        let canonical_bytes = tokio::fs::read(&canonical_path).await?;
        Ok(Some(serde_json::from_slice(&canonical_bytes)?))
    }

    /// Load a card and reconcile any Syncthing-style `.sync-conflict-*.json`
    /// siblings into the canonical file. Callers must hold the per-card-id
    /// lock (see `lock_for`) — load may write the merged result back to disk
    /// and delete conflict siblings.
    pub async fn load(
        &self,
        source_language: &str,
        target_language: &str,
        lemma_slug: &str,
    ) -> anyhow::Result<Option<Card>> {
        let Some(mut base) = self
            .load_canonical(source_language, target_language, lemma_slug)
            .await?
        else {
            return Ok(None);
        };

        let deck_dir = self.deck_dir(source_language, target_language);
        let canonical_file_name = format!("{lemma_slug}.json");
        let file_name_prefix = format!("{lemma_slug}.");
        let expected_id = card_id(source_language, target_language, lemma_slug);

        let accepted = self
            .scan_conflict_siblings(
                &deck_dir,
                &canonical_file_name,
                &file_name_prefix,
                source_language,
                target_language,
                &expected_id,
            )
            .await;

        if accepted.is_empty() {
            return Ok(Some(base));
        }

        for (_, card) in &accepted {
            base.merge(card.clone());
        }

        // Merge writeback is a normalization triggered by `load`, not a
        // user-driven card change. Silent so sync isn't woken just because
        // a conflict sibling happened to be present.
        self.save_without_wake(&base, source_language, target_language)
            .await?;

        for (path, _) in accepted {
            if let Err(err) = tokio::fs::remove_file(&path).await {
                log::warn!("Failed to delete conflict sibling {path:?}: {err}");
            }
        }

        Ok(Some(base))
    }

    /// Resolve the reader-side familiarity scalar for many lemma slugs at
    /// once. Returns a slug→scalar map containing only the slugs that should
    /// render (i.e. those whose [`familiarity_from`] yielded `Some`); an
    /// absent slug means the word is dormant. Warm slugs are served from the
    /// in-memory cache; cold slugs are read once from their canonical card
    /// file (no deck-dir scan), concurrently, and memoized. Reads are pure, so
    /// no per-card lock is required.
    pub async fn familiarities(
        &self,
        source_language: &str,
        target_language: &str,
        slugs: &[String],
    ) -> HashMap<String, f32> {
        // Partition into cache hits (resolved immediately) and misses.
        let mut resolved: HashMap<String, Option<f32>> = HashMap::with_capacity(slugs.len());
        let mut misses: Vec<String> = Vec::new();
        {
            let cache = self.fam_cache.read().unwrap();
            for slug in slugs {
                let id = card_id(source_language, target_language, slug);
                match cache.get(&id) {
                    Some(fam) => {
                        resolved.insert(slug.clone(), *fam);
                    }
                    None => misses.push(slug.clone()),
                }
            }
        }

        if !misses.is_empty() {
            let loaded =
                futures_util::future::join_all(misses.into_iter().map(|slug| async move {
                    match self
                        .load_canonical(source_language, target_language, &slug)
                        .await
                    {
                        Ok(card) => {
                            let fam =
                                familiarity_from(card.as_ref().and_then(|c| c.anki_data.as_ref()));
                            (slug, fam, true)
                        }
                        // Don't poison the cache on a transient read error:
                        // render this word as never-synced but leave it a miss
                        // so the next render retries from disk.
                        Err(_) => (slug, Some(0.0), false),
                    }
                }))
                .await;

            let mut cache = self.fam_cache.write().unwrap();
            for (slug, fam, cacheable) in loaded {
                if cacheable {
                    let id = card_id(source_language, target_language, &slug);
                    cache.insert(id, fam);
                }
                resolved.insert(slug, fam);
            }
        }

        resolved
            .into_iter()
            .filter_map(|(slug, fam)| fam.map(|f| (slug, f)))
            .collect()
    }

    /// Drop the cached familiarity for one card so the next read repopulates
    /// it from disk. Use when a card file changes outside our own `save`
    /// (e.g. Syncthing delivers an update from another device).
    pub fn invalidate_familiarity(
        &self,
        source_language: &str,
        target_language: &str,
        lemma_slug: &str,
    ) {
        let id = card_id(source_language, target_language, lemma_slug);
        self.fam_cache.write().unwrap().remove(&id);
    }

    async fn scan_conflict_siblings(
        &self,
        deck_dir: &Path,
        canonical_file_name: &str,
        file_name_prefix: &str,
        source_language: &str,
        target_language: &str,
        expected_id: &str,
    ) -> Vec<(PathBuf, Card)> {
        let mut accepted: Vec<(PathBuf, Card)> = Vec::new();
        let mut read_dir = match tokio::fs::read_dir(deck_dir).await {
            Ok(rd) => rd,
            Err(err) => {
                log::warn!("Failed to read deck dir {deck_dir:?} for conflict scan: {err}");
                return accepted;
            }
        };

        loop {
            let entry = match read_dir.next_entry().await {
                Ok(Some(e)) => e,
                Ok(None) => break,
                Err(err) => {
                    log::warn!("Error iterating deck dir {deck_dir:?}: {err}");
                    break;
                }
            };
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name == canonical_file_name {
                continue;
            }
            if !name.starts_with(file_name_prefix) || !name.ends_with(".json") {
                continue;
            }
            let bytes = match tokio::fs::read(&path).await {
                Ok(b) => b,
                Err(err) => {
                    log::warn!("Failed to read conflict sibling {path:?}: {err}");
                    continue;
                }
            };
            let card: Card = match serde_json::from_slice(&bytes) {
                Ok(c) => c,
                Err(err) => {
                    log::warn!("Failed to parse conflict sibling {path:?} as Card: {err}");
                    continue;
                }
            };
            let sibling_id = card_id(
                source_language,
                target_language,
                &lemma_slug(&card.lemma),
            );
            if sibling_id != expected_id {
                log::warn!(
                    "Conflict sibling {path:?} has derived id {sibling_id}, expected {expected_id}; skipping"
                );
                continue;
            }
            accepted.push((path, card));
        }

        accepted.sort_by(|a, b| a.0.cmp(&b.0));
        accepted
    }

    /// Enumerate `<src>-<tgt>` deck directories under `cards/`. Names lacking
    /// a `-` are silently skipped. Missing root returns `Ok(vec![])`. Result is
    /// sorted ascending.
    pub async fn list_pairs(&self) -> anyhow::Result<Vec<(String, String)>> {
        let mut read_dir = match tokio::fs::read_dir(&self.root).await {
            Ok(rd) => rd,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(err) => return Err(err.into()),
        };

        let mut pairs: Vec<(String, String)> = Vec::new();
        loop {
            let entry = match read_dir.next_entry().await? {
                Some(e) => e,
                None => break,
            };
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            let name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };
            let Some((src, tgt)) = name.split_once('-') else {
                continue;
            };
            pairs.push((src.to_owned(), tgt.to_owned()));
        }
        pairs.sort();
        Ok(pairs)
    }

    /// Enumerate lemma slugs for the given pair's deck dir. Skips Syncthing
    /// `.sync-conflict-*.json` siblings and any non-`.json` files. Missing
    /// deck dir returns `Ok(vec![])`. Result is sorted ascending.
    pub async fn list_cards_in_pair(
        &self,
        source_language: &str,
        target_language: &str,
    ) -> anyhow::Result<Vec<String>> {
        let deck = self.deck_dir(source_language, target_language);
        let mut read_dir = match tokio::fs::read_dir(&deck).await {
            Ok(rd) => rd,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(err) => return Err(err.into()),
        };

        let mut out: Vec<String> = Vec::new();
        loop {
            let entry = match read_dir.next_entry().await? {
                Some(e) => e,
                None => break,
            };
            let name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };
            if name.contains(".sync-conflict-") {
                continue;
            }
            let Some(stem) = name.strip_suffix(".json") else {
                continue;
            };
            out.push(stem.to_owned());
        }
        out.sort();
        Ok(out)
    }

    /// Persist a card to disk and wake any sync task listening on
    /// `change_notify`. Use this from user-driven write paths (translation
    /// completion, backfill, on-disk edits).
    pub async fn save(
        &self,
        card: &Card,
        source_language: &str,
        target_language: &str,
    ) -> anyhow::Result<()> {
        self.save_inner(card, source_language, target_language, true)
            .await
    }

    /// Persist a card to disk WITHOUT firing the change-notify wake. Use
    /// from code paths that themselves run inside a sync pass (or otherwise
    /// shouldn't self-trigger one), e.g. sync_pass writing back pulled
    /// `anki_data`, or `load` normalizing a conflict merge.
    pub async fn save_without_wake(
        &self,
        card: &Card,
        source_language: &str,
        target_language: &str,
    ) -> anyhow::Result<()> {
        self.save_inner(card, source_language, target_language, false)
            .await
    }

    async fn save_inner(
        &self,
        card: &Card,
        source_language: &str,
        target_language: &str,
        notify: bool,
    ) -> anyhow::Result<()> {
        let deck = self.deck_dir(source_language, target_language);
        tokio::fs::create_dir_all(&deck).await?;

        // `card.lemma` is already NFC + apostrophe + whitespace-normalized
        // by extract_card_updates (the only writer), but case-preserving.
        // Lowercase before slugging so the filename matches the slug pipeline.
        let slug = lemma_slug(&card.lemma.to_lowercase());
        let file_name = format!("{slug}.json");
        let canonical = deck.join(&file_name);
        let temp = deck.join(format!("{file_name}~{}", create_random_string(8)));

        let bytes = serde_json::to_vec_pretty(card)?;
        tokio::fs::write(&temp, bytes).await?;
        tokio::fs::rename(&temp, &canonical).await?;

        // Keep the reader-side familiarity cache fresh without a re-read — we
        // hold the authoritative card. Covers card creation, Anki sync
        // writeback, and the conflict-merge writeback inside `load`.
        let id = card_id(source_language, target_language, &slug);
        self.fam_cache
            .write()
            .unwrap()
            .insert(id, familiarity_from(card.anki_data.as_ref()));

        if notify {
            self.change_notify.notify_one();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{card::Card, test_utils::TempDir};

    fn sample_card() -> Card {
        let mut translations: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        translations.insert("verb".into(), vec!["мочь".into()]);
        Card {
            version: 2,
            id: "flts_spa_rus_poder".into(),
            lemma: "poder".into(),
            translations,
            examples: vec![],
            anki_data: None,
        }
    }

    #[tokio::test]
    async fn save_creates_file_at_expected_path() {
        let tmp = TempDir::new("flts_card_save");
        let store = LibraryCardStore::new(&tmp.path);
        store.save(&sample_card(), "spa", "rus").await.unwrap();
        let expected = tmp
            .path
            .join("cards")
            .join("spa-rus")
            .join("poder.json");
        assert!(expected.exists(), "expected card at {expected:?}");
    }

    #[tokio::test]
    async fn save_writes_pretty_json() {
        let tmp = TempDir::new("flts_card_pretty");
        let store = LibraryCardStore::new(&tmp.path);
        store.save(&sample_card(), "spa", "rus").await.unwrap();
        let path = tmp
            .path
            .join("cards")
            .join("spa-rus")
            .join("poder.json");
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.starts_with("{\n"), "expected pretty JSON, got: {body}");
        assert!(body.contains("\"version\": 2"));
        assert!(body.contains("\"anki_data\": null"));
    }

    #[tokio::test]
    async fn save_fires_change_notify() {
        let tmp = TempDir::new("flts_card_notify");
        let store = LibraryCardStore::new(&tmp.path);
        let notify = store.change_notify();
        let waiter = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(2), notify.notified())
                .await
                .expect("change_notify must fire within timeout")
        });
        store.save(&sample_card(), "spa", "rus").await.unwrap();
        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn save_without_wake_does_not_fire_change_notify() {
        let tmp = TempDir::new("flts_card_silent");
        let store = LibraryCardStore::new(&tmp.path);
        let notify = store.change_notify();
        store
            .save_without_wake(&sample_card(), "spa", "rus")
            .await
            .unwrap();
        // notify_one is queued (max 1 permit). If save_without_wake wakes
        // erroneously, this notified() returns immediately; otherwise it
        // pends until the timeout elapses.
        let pending = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            notify.notified(),
        )
        .await;
        assert!(
            pending.is_err(),
            "save_without_wake must not fire change_notify"
        );
    }

    #[tokio::test]
    async fn save_leaves_no_temp_files() {
        let tmp = TempDir::new("flts_card_no_temp");
        let store = LibraryCardStore::new(&tmp.path);
        store.save(&sample_card(), "spa", "rus").await.unwrap();
        let deck = tmp.path.join("cards").join("spa-rus");
        let entries: Vec<_> = std::fs::read_dir(&deck)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(
            entries.iter().all(|n| !n.contains('~')),
            "found stray temp file in {entries:?}"
        );
    }

    #[tokio::test]
    async fn load_returns_none_for_missing() {
        let tmp = TempDir::new("flts_card_missing");
        let store = LibraryCardStore::new(&tmp.path);
        let card = store.load("spa", "rus", "poder").await.unwrap();
        assert!(card.is_none());
    }

    #[tokio::test]
    async fn load_round_trips_saved_card() {
        let tmp = TempDir::new("flts_card_roundtrip");
        let store = LibraryCardStore::new(&tmp.path);
        let original = sample_card();
        store.save(&original, "spa", "rus").await.unwrap();
        let loaded = store
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(original, loaded);
    }

    #[tokio::test]
    async fn per_card_lock_is_per_id() {
        let tmp = TempDir::new("flts_card_lock");
        let store = LibraryCardStore::new(&tmp.path);
        let a1 = store.lock_for("flts_spa_rus_poder").await;
        let a2 = store.lock_for("flts_spa_rus_poder").await;
        let b = store.lock_for("flts_spa_rus_comer").await;
        assert!(Arc::ptr_eq(&a1, &a2), "same id should yield same Arc");
        assert!(
            !Arc::ptr_eq(&a1, &b),
            "different ids should yield distinct Arcs"
        );
    }

    use crate::card::Example;
    use uuid::Uuid;

    fn card_with(
        lemma: &str,
        part_of_speech: &str,
        translations: Vec<&str>,
        examples: Vec<Example>,
    ) -> Card {
        let slug = lemma_slug(lemma);
        let mut by_pos: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        by_pos.insert(
            part_of_speech.into(),
            translations.into_iter().map(String::from).collect(),
        );
        Card {
            version: 2,
            id: format!("flts_spa_rus_{slug}"),
            lemma: lemma.into(),
            translations: by_pos,
            examples,
            anki_data: None,
        }
    }

    fn example(
        book: Uuid,
        chapter: usize,
        paragraph: usize,
        source: &str,
        translation: &str,
    ) -> Example {
        Example {
            source: source.into(),
            translation: translation.into(),
            book_id: book,
            chapter,
            paragraph,
        }
    }

    async fn write_pretty(path: &Path, card: &Card) {
        let bytes = serde_json::to_vec_pretty(card).unwrap();
        tokio::fs::write(path, bytes).await.unwrap();
    }

    fn deck_entries(deck: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(deck)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn load_returns_canonical_when_no_siblings() {
        let tmp = TempDir::new("flts_load_no_siblings");
        let store = LibraryCardStore::new(&tmp.path);
        store.save(&sample_card(), "spa", "rus").await.unwrap();

        let loaded = store
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(loaded, sample_card());

        let deck = tmp.path.join("cards").join("spa-rus");
        assert_eq!(deck_entries(&deck), vec!["poder.json"]);
    }

    #[tokio::test]
    async fn load_merges_single_sync_conflict_sibling() {
        let tmp = TempDir::new("flts_load_single_conflict");
        let store = LibraryCardStore::new(&tmp.path);
        let book = Uuid::new_v4();
        let canonical = card_with(
            "poder",
            "verb",
            vec!["мочь"],
            vec![example(book, 0, 0, "puedo", "могу")],
        );
        store.save(&canonical, "spa", "rus").await.unwrap();

        let deck = tmp.path.join("cards").join("spa-rus");
        let conflict_path = deck.join("poder.sync-conflict-20260520-153912-XYZ.json");
        let conflict = card_with(
            "poder",
            "verb",
            vec!["уметь"],
            vec![example(book, 1, 5, "pueden", "могут")],
        );
        write_pretty(&conflict_path, &conflict).await;

        let merged = store
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(merged.translations_flat(), vec!["мочь", "уметь"]);
        assert_eq!(merged.examples.len(), 2);

        assert!(!conflict_path.exists(), "conflict sibling must be deleted");
        assert_eq!(deck_entries(&deck), vec!["poder.json"]);

        let on_disk: Card =
            serde_json::from_slice(&tokio::fs::read(deck.join("poder.json")).await.unwrap())
                .unwrap();
        assert_eq!(on_disk, merged);
    }

    #[tokio::test]
    async fn load_merges_multiple_sync_conflict_siblings() {
        let tmp = TempDir::new("flts_load_many_conflicts");
        let store = LibraryCardStore::new(&tmp.path);
        let book = Uuid::new_v4();
        let canonical = card_with(
            "poder",
            "verb",
            vec!["мочь"],
            vec![example(book, 0, 0, "a", "1")],
        );
        store.save(&canonical, "spa", "rus").await.unwrap();
        let deck = tmp.path.join("cards").join("spa-rus");

        for (suffix, t, p) in [
            ("alpha", "уметь", 1usize),
            ("beta", "иметь возможность", 2usize),
            ("gamma", "сметь", 3usize),
        ] {
            let p_card = card_with(
                "poder",
                "verb",
                vec![t],
                vec![example(book, 0, p, &format!("s{p}"), &format!("t{p}"))],
            );
            write_pretty(
                &deck.join(format!("poder.sync-conflict-20260520-{suffix}.json")),
                &p_card,
            )
            .await;
        }

        let merged = store
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(
            merged.translations_flat(),
            vec!["мочь", "уметь", "иметь возможность", "сметь"]
        );
        assert_eq!(merged.examples.len(), 4);
        assert_eq!(deck_entries(&deck), vec!["poder.json"]);
    }

    #[tokio::test]
    async fn load_ignores_sibling_with_mismatched_derived_id() {
        let tmp = TempDir::new("flts_load_mismatch_id");
        let store = LibraryCardStore::new(&tmp.path);
        let book = Uuid::new_v4();
        store
            .save(
                &card_with(
                    "poder",
                    "verb",
                    vec!["мочь"],
                    vec![example(book, 0, 0, "a", "1")],
                ),
                "spa",
                "rus",
            )
            .await
            .unwrap();

        let deck = tmp.path.join("cards").join("spa-rus");
        let foreign_path = deck.join("poder.sync-conflict-X.json");
        // Foreign card masquerades under the conflict-name pattern but its lemma
        // (`comer`) would derive id `flts_spa_rus_comer_verb`, not `poder_verb`.
        let foreign = card_with(
            "comer",
            "verb",
            vec!["есть"],
            vec![example(book, 9, 9, "como", "ем")],
        );
        write_pretty(&foreign_path, &foreign).await;

        let loaded = store
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(loaded.translations_flat(), vec!["мочь"]);
        assert_eq!(loaded.examples.len(), 1);

        assert!(
            foreign_path.exists(),
            "mismatched sibling must NOT be deleted"
        );
    }

    #[tokio::test]
    async fn load_ignores_unrelated_files_in_deck() {
        let tmp = TempDir::new("flts_load_unrelated");
        let store = LibraryCardStore::new(&tmp.path);
        let book = Uuid::new_v4();
        store
            .save(
                &card_with(
                    "poder",
                    "verb",
                    vec!["мочь"],
                    vec![example(book, 0, 0, "a", "1")],
                ),
                "spa",
                "rus",
            )
            .await
            .unwrap();
        store
            .save(
                &card_with(
                    "comer",
                    "verb",
                    vec!["есть"],
                    vec![example(book, 0, 1, "b", "2")],
                ),
                "spa",
                "rus",
            )
            .await
            .unwrap();

        let deck = tmp.path.join("cards").join("spa-rus");
        let comer_conflict = deck.join("comer.sync-conflict-X.json");
        write_pretty(
            &comer_conflict,
            &card_with(
                "comer",
                "verb",
                vec!["кушать"],
                vec![example(book, 1, 1, "c", "3")],
            ),
        )
        .await;

        store.load("spa", "rus", "poder").await.unwrap();
        assert!(
            comer_conflict.exists(),
            "comer's conflict file must be untouched by poder load"
        );
        let poder: Card =
            serde_json::from_slice(&tokio::fs::read(deck.join("poder.json")).await.unwrap())
                .unwrap();
        assert_eq!(poder.translations_flat(), vec!["мочь"]);
    }

    #[tokio::test]
    async fn load_skips_corrupt_sibling_without_deleting() {
        let tmp = TempDir::new("flts_load_corrupt");
        let store = LibraryCardStore::new(&tmp.path);
        let book = Uuid::new_v4();
        store
            .save(
                &card_with(
                    "poder",
                    "verb",
                    vec!["мочь"],
                    vec![example(book, 0, 0, "a", "1")],
                ),
                "spa",
                "rus",
            )
            .await
            .unwrap();

        let deck = tmp.path.join("cards").join("spa-rus");
        let corrupt_path = deck.join("poder.sync-conflict-corrupt.json");
        tokio::fs::write(&corrupt_path, b"{not valid json")
            .await
            .unwrap();

        let loaded = store
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(loaded.translations_flat(), vec!["мочь"]);
        assert_eq!(loaded.examples.len(), 1);
        assert!(corrupt_path.exists(), "corrupt sibling must NOT be deleted");
    }

    #[tokio::test]
    async fn load_leaves_no_stray_temp_files_after_merge() {
        let tmp = TempDir::new("flts_load_no_temp");
        let store = LibraryCardStore::new(&tmp.path);
        let book = Uuid::new_v4();
        store
            .save(
                &card_with(
                    "poder",
                    "verb",
                    vec!["мочь"],
                    vec![example(book, 0, 0, "a", "1")],
                ),
                "spa",
                "rus",
            )
            .await
            .unwrap();
        let deck = tmp.path.join("cards").join("spa-rus");
        write_pretty(
            &deck.join("poder.sync-conflict-X.json"),
            &card_with(
                "poder",
                "verb",
                vec!["уметь"],
                vec![example(book, 1, 1, "b", "2")],
            ),
        )
        .await;

        store.load("spa", "rus", "poder").await.unwrap();

        let entries = deck_entries(&deck);
        assert!(
            entries.iter().all(|n| !n.contains('~')),
            "found stray temp file in {entries:?}"
        );
        assert_eq!(entries, vec!["poder.json"]);
    }

    #[tokio::test]
    async fn load_returns_none_when_canonical_absent_even_with_siblings() {
        let tmp = TempDir::new("flts_load_canonical_absent");
        let store = LibraryCardStore::new(&tmp.path);
        let book = Uuid::new_v4();
        // Bootstrap the deck dir via a save we then remove.
        store
            .save(
                &card_with(
                    "poder",
                    "verb",
                    vec!["мочь"],
                    vec![example(book, 0, 0, "a", "1")],
                ),
                "spa",
                "rus",
            )
            .await
            .unwrap();
        let deck = tmp.path.join("cards").join("spa-rus");
        let canonical = deck.join("poder.json");
        let conflict_path = deck.join("poder.sync-conflict-X.json");
        write_pretty(
            &conflict_path,
            &card_with(
                "poder",
                "verb",
                vec!["уметь"],
                vec![example(book, 1, 1, "b", "2")],
            ),
        )
        .await;
        tokio::fs::remove_file(&canonical).await.unwrap();

        let loaded = store.load("spa", "rus", "poder").await.unwrap();
        assert!(loaded.is_none(), "expected None when canonical is absent");
        assert!(
            conflict_path.exists(),
            "sibling must be untouched when canonical is absent"
        );
    }

    #[tokio::test]
    async fn list_cards_in_pair_returns_empty_when_deck_missing() {
        let tmp = TempDir::new("flts_list_cards_empty");
        let store = LibraryCardStore::new(&tmp.path);
        let cards = store.list_cards_in_pair("spa", "rus").await.unwrap();
        assert!(cards.is_empty(), "missing deck dir must yield empty list");
    }

    #[tokio::test]
    async fn list_cards_in_pair_returns_lemma_slugs() {
        let tmp = TempDir::new("flts_list_cards");
        let store = LibraryCardStore::new(&tmp.path);
        store
            .save(
                &card_with("poder", "verb", vec!["мочь"], vec![]),
                "spa",
                "rus",
            )
            .await
            .unwrap();
        store
            .save(
                &card_with("comer", "verb", vec!["есть"], vec![]),
                "spa",
                "rus",
            )
            .await
            .unwrap();

        // Seed a Syncthing conflict sibling — must be skipped.
        let deck = tmp.path.join("cards").join("spa-rus");
        std::fs::write(deck.join("poder.sync-conflict-20260520-X.json"), b"{}").unwrap();
        // And a stray non-JSON file — also skipped.
        std::fs::write(deck.join("README"), b"ignore").unwrap();

        let cards = store.list_cards_in_pair("spa", "rus").await.unwrap();
        assert_eq!(
            cards,
            vec!["comer".to_owned(), "poder".to_owned()],
            "expected sorted lemma slugs, got {cards:?}"
        );
    }

    #[tokio::test]
    async fn list_pairs_returns_empty_when_root_missing() {
        let tmp = TempDir::new("flts_list_pairs_empty");
        let store = LibraryCardStore::new(&tmp.path);
        let pairs = store.list_pairs().await.unwrap();
        assert!(
            pairs.is_empty(),
            "expected empty list when cards dir is missing"
        );
    }

    #[tokio::test]
    async fn list_pairs_returns_pair_for_each_deck_dir() {
        let tmp = TempDir::new("flts_list_pairs");
        let store = LibraryCardStore::new(&tmp.path);
        // Bootstrap two deck dirs via save().
        store.save(&sample_card(), "spa", "rus").await.unwrap();
        store
            .save(
                &card_with("hello", "noun", vec!["привет"], vec![]),
                "eng",
                "rus",
            )
            .await
            .unwrap();
        // Add a non-pair directory and a stray file that must be ignored.
        std::fs::create_dir(tmp.path.join("cards").join("not_a_pair")).unwrap();
        std::fs::write(tmp.path.join("cards").join("README"), b"ignore me").unwrap();

        let pairs = store.list_pairs().await.unwrap();
        assert_eq!(
            pairs,
            vec![
                ("eng".to_owned(), "rus".to_owned()),
                ("spa".to_owned(), "rus".to_owned()),
            ],
            "expected sorted pairs, got {pairs:?}"
        );
    }

    use crate::card::{AnkiData, AnkiState};

    fn card_with_anki(lemma: &str, state: AnkiState, fsrs_stability: Option<f64>) -> Card {
        let slug = lemma_slug(lemma);
        let mut translations: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        translations.insert("verb".into(), vec!["x".into()]);
        Card {
            version: 2,
            id: format!("flts_spa_rus_{slug}"),
            lemma: lemma.into(),
            translations,
            examples: vec![],
            anki_data: Some(AnkiData {
                state,
                interval_days: None,
                ease_factor: None,
                fsrs_difficulty: None,
                fsrs_stability,
            }),
        }
    }

    fn never_synced_card(lemma: &str) -> Card {
        let slug = lemma_slug(lemma);
        let mut translations: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        translations.insert("verb".into(), vec!["x".into()]);
        Card {
            version: 2,
            id: format!("flts_spa_rus_{slug}"),
            lemma: lemma.into(),
            translations,
            examples: vec![],
            anki_data: None,
        }
    }

    // A mature Active card (stability == MATURE_DAYS) collapses to familiarity
    // 1.0 by the `familiarity_from` contract — handy as an exact assertion.
    #[tokio::test]
    async fn familiarities_maps_states_like_per_word_path() {
        let tmp = TempDir::new("flts_fam_states");
        // Write cards through one store, then read cold through a fresh store
        // so the in-memory cache starts empty and `familiarities` exercises
        // the `load_canonical` + `familiarity_from` path.
        {
            let writer = LibraryCardStore::new(&tmp.path);
            writer
                .save_without_wake(&never_synced_card("poder"), "spa", "rus")
                .await
                .unwrap();
            writer
                .save_without_wake(
                    &card_with_anki("comer", AnkiState::Active, Some(90.0)),
                    "spa",
                    "rus",
                )
                .await
                .unwrap();
            writer
                .save_without_wake(
                    &card_with_anki("vivir", AnkiState::Suspended, None),
                    "spa",
                    "rus",
                )
                .await
                .unwrap();
        }

        let store = LibraryCardStore::new(&tmp.path);
        let slugs = vec![
            "poder".to_string(),
            "comer".to_string(),
            "vivir".to_string(),
            "ausente".to_string(),
        ];
        let fam = store.familiarities("spa", "rus", &slugs).await;

        assert_eq!(fam.get("poder").copied(), Some(0.0), "never-synced → 0.0");
        assert_eq!(fam.get("comer").copied(), Some(1.0), "mature active → 1.0");
        assert!(!fam.contains_key("vivir"), "suspended → dormant → absent");
        assert_eq!(fam.get("ausente").copied(), Some(0.0), "no file → 0.0");
    }

    #[tokio::test]
    async fn save_populates_cache_and_invalidate_forces_reread() {
        let tmp = TempDir::new("flts_fam_cache");
        let store = LibraryCardStore::new(&tmp.path);

        // save updates the cache without a re-read.
        store
            .save_without_wake(
                &card_with_anki("poder", AnkiState::Active, Some(90.0)),
                "spa",
                "rus",
            )
            .await
            .unwrap();
        let fam = store
            .familiarities("spa", "rus", &["poder".to_string()])
            .await;
        assert_eq!(fam.get("poder").copied(), Some(1.0));

        // Mutate the on-disk file behind the cache's back: still served stale.
        write_pretty(
            &store.card_path("spa", "rus", "poder"),
            &card_with_anki("poder", AnkiState::Suspended, None),
        )
        .await;
        let fam = store
            .familiarities("spa", "rus", &["poder".to_string()])
            .await;
        assert_eq!(
            fam.get("poder").copied(),
            Some(1.0),
            "cache hit should not re-read disk"
        );

        // After invalidation the next read reflects disk: dormant → absent.
        store.invalidate_familiarity("spa", "rus", "poder");
        let fam = store
            .familiarities("spa", "rus", &["poder".to_string()])
            .await;
        assert!(
            !fam.contains_key("poder"),
            "invalidated entry must re-read and show dormant"
        );
    }

    #[tokio::test]
    async fn load_canonical_ignores_conflict_siblings() {
        let tmp = TempDir::new("flts_load_canonical");
        let store = LibraryCardStore::new(&tmp.path);
        let book = Uuid::new_v4();
        store
            .save(
                &card_with("poder", "verb", vec!["мочь"], vec![example(book, 0, 0, "a", "1")]),
                "spa",
                "rus",
            )
            .await
            .unwrap();

        let deck = tmp.path.join("cards").join("spa-rus");
        let conflict_path = deck.join("poder.sync-conflict-20260520-XYZ.json");
        write_pretty(
            &conflict_path,
            &card_with("poder", "verb", vec!["уметь"], vec![example(book, 1, 5, "b", "2")]),
        )
        .await;

        // Canonical read returns the single file untouched, sibling intact.
        let only = store
            .load_canonical("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(only.translations_flat(), vec!["мочь"]);
        assert!(
            conflict_path.exists(),
            "load_canonical must not touch conflict siblings"
        );

        // The full `load` still reconciles and cleans up the sibling.
        let merged = store
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(merged.translations_flat(), vec!["мочь", "уметь"]);
        assert!(!conflict_path.exists(), "load must reconcile siblings");
    }
}
