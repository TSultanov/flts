use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::sync::Mutex;

use crate::{
    book::serialization::create_random_string,
    card::{Card, card_id, lemma_slug, part_of_speech_slug},
};

pub struct LibraryCardStore {
    root: PathBuf,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl LibraryCardStore {
    pub fn new(library_root: &Path) -> Self {
        Self {
            root: library_root.join("cards"),
            locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn deck_dir(&self, source_language: &str, target_language: &str) -> PathBuf {
        self.root.join(format!("{source_language}-{target_language}"))
    }

    pub fn card_path(
        &self,
        source_language: &str,
        target_language: &str,
        lemma_slug: &str,
        pos_slug: &str,
    ) -> PathBuf {
        self.deck_dir(source_language, target_language)
            .join(format!("{lemma_slug}_{pos_slug}.json"))
    }

    pub async fn lock_for(&self, id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        locks
            .entry(id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
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
        pos_slug: &str,
    ) -> anyhow::Result<Option<Card>> {
        let canonical_path =
            self.card_path(source_language, target_language, lemma_slug, pos_slug);
        if !tokio::fs::try_exists(&canonical_path).await? {
            return Ok(None);
        }
        let canonical_bytes = tokio::fs::read(&canonical_path).await?;
        let mut base: Card = serde_json::from_slice(&canonical_bytes)?;

        let deck_dir = self.deck_dir(source_language, target_language);
        let canonical_file_name = format!("{lemma_slug}_{pos_slug}.json");
        let file_name_prefix = format!("{lemma_slug}_{pos_slug}.");
        let expected_id = card_id(source_language, target_language, lemma_slug, pos_slug);

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

        self.save(&base, source_language, target_language).await?;

        for (path, _) in accepted {
            if let Err(err) = tokio::fs::remove_file(&path).await {
                log::warn!("Failed to delete conflict sibling {path:?}: {err}");
            }
        }

        Ok(Some(base))
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
                &part_of_speech_slug(&card.part_of_speech),
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

    pub async fn save(&self, card: &Card, source_language: &str, target_language: &str) -> anyhow::Result<()> {
        let deck = self.deck_dir(source_language, target_language);
        tokio::fs::create_dir_all(&deck).await?;

        let slug = lemma_slug(&card.lemma);
        let pos_slug = part_of_speech_slug(&card.part_of_speech);
        let file_name = format!("{slug}_{pos_slug}.json");
        let canonical = deck.join(&file_name);
        let temp = deck.join(format!("{file_name}~{}", create_random_string(8)));

        let bytes = serde_json::to_vec_pretty(card)?;
        tokio::fs::write(&temp, bytes).await?;
        tokio::fs::rename(&temp, &canonical).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{card::Card, test_utils::TempDir};

    fn sample_card() -> Card {
        Card {
            version: 1,
            id: "flts_spa_rus_poder_verb".into(),
            lemma: "poder".into(),
            part_of_speech: "verb".into(),
            translations: vec!["мочь".into()],
            examples: vec![],
            anki_data: None,
        }
    }

    #[tokio::test]
    async fn save_creates_file_at_expected_path() {
        let tmp = TempDir::new("flts_card_save");
        let store = LibraryCardStore::new(&tmp.path);
        store.save(&sample_card(), "spa", "rus").await.unwrap();
        let expected = tmp.path.join("cards").join("spa-rus").join("poder_verb.json");
        assert!(expected.exists(), "expected card at {expected:?}");
    }

    #[tokio::test]
    async fn save_writes_pretty_json() {
        let tmp = TempDir::new("flts_card_pretty");
        let store = LibraryCardStore::new(&tmp.path);
        store.save(&sample_card(), "spa", "rus").await.unwrap();
        let path = tmp.path.join("cards").join("spa-rus").join("poder_verb.json");
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.starts_with("{\n"), "expected pretty JSON, got: {body}");
        assert!(body.contains("\"version\": 1"));
        assert!(body.contains("\"anki_data\": null"));
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
        let card = store.load("spa", "rus", "poder", "verb").await.unwrap();
        assert!(card.is_none());
    }

    #[tokio::test]
    async fn load_round_trips_saved_card() {
        let tmp = TempDir::new("flts_card_roundtrip");
        let store = LibraryCardStore::new(&tmp.path);
        let original = sample_card();
        store.save(&original, "spa", "rus").await.unwrap();
        let loaded = store
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(original, loaded);
    }

    #[tokio::test]
    async fn per_card_lock_is_per_id() {
        let tmp = TempDir::new("flts_card_lock");
        let store = LibraryCardStore::new(&tmp.path);
        let a1 = store.lock_for("flts_spa_rus_poder_verb").await;
        let a2 = store.lock_for("flts_spa_rus_poder_verb").await;
        let b = store.lock_for("flts_spa_rus_poder_noun").await;
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
        Card {
            version: 1,
            id: format!("flts_spa_rus_{slug}_{part_of_speech}"),
            lemma: lemma.into(),
            part_of_speech: part_of_speech.into(),
            translations: translations.into_iter().map(String::from).collect(),
            examples,
            anki_data: None,
        }
    }

    fn example(book: Uuid, chapter: usize, paragraph: usize, source: &str, translation: &str) -> Example {
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
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(loaded, sample_card());

        let deck = tmp.path.join("cards").join("spa-rus");
        assert_eq!(deck_entries(&deck), vec!["poder_verb.json"]);
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
        let conflict_path = deck.join("poder_verb.sync-conflict-20260520-153912-XYZ.json");
        let conflict = card_with(
            "poder",
            "verb",
            vec!["уметь"],
            vec![example(book, 1, 5, "pueden", "могут")],
        );
        write_pretty(&conflict_path, &conflict).await;

        let merged = store
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(merged.translations, vec!["мочь", "уметь"]);
        assert_eq!(merged.examples.len(), 2);

        assert!(!conflict_path.exists(), "conflict sibling must be deleted");
        assert_eq!(deck_entries(&deck), vec!["poder_verb.json"]);

        let on_disk: Card =
            serde_json::from_slice(&tokio::fs::read(deck.join("poder_verb.json")).await.unwrap()).unwrap();
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
                &deck.join(format!("poder_verb.sync-conflict-20260520-{suffix}.json")),
                &p_card,
            )
            .await;
        }

        let merged = store
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(
            merged.translations,
            vec!["мочь", "уметь", "иметь возможность", "сметь"]
        );
        assert_eq!(merged.examples.len(), 4);
        assert_eq!(deck_entries(&deck), vec!["poder_verb.json"]);
    }

    #[tokio::test]
    async fn load_ignores_sibling_with_mismatched_derived_id() {
        let tmp = TempDir::new("flts_load_mismatch_id");
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
        let foreign_path = deck.join("poder_verb.sync-conflict-X.json");
        // Foreign card masquerades under the conflict-name pattern but its lemma
        // (`comer`) would derive id `flts_spa_rus_comer_verb`, not `poder_verb`.
        let foreign = card_with("comer", "verb", vec!["есть"], vec![example(book, 9, 9, "como", "ем")]);
        write_pretty(&foreign_path, &foreign).await;

        let loaded = store
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(loaded.translations, vec!["мочь"]);
        assert_eq!(loaded.examples.len(), 1);

        assert!(foreign_path.exists(), "mismatched sibling must NOT be deleted");
    }

    #[tokio::test]
    async fn load_ignores_unrelated_files_in_deck() {
        let tmp = TempDir::new("flts_load_unrelated");
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
        store
            .save(
                &card_with("comer", "verb", vec!["есть"], vec![example(book, 0, 1, "b", "2")]),
                "spa",
                "rus",
            )
            .await
            .unwrap();

        let deck = tmp.path.join("cards").join("spa-rus");
        let comer_conflict = deck.join("comer_verb.sync-conflict-X.json");
        write_pretty(
            &comer_conflict,
            &card_with("comer", "verb", vec!["кушать"], vec![example(book, 1, 1, "c", "3")]),
        )
        .await;

        store.load("spa", "rus", "poder", "verb").await.unwrap();
        assert!(comer_conflict.exists(), "comer's conflict file must be untouched by poder load");
        let poder: Card =
            serde_json::from_slice(&tokio::fs::read(deck.join("poder_verb.json")).await.unwrap()).unwrap();
        assert_eq!(poder.translations, vec!["мочь"]);
    }

    #[tokio::test]
    async fn load_skips_corrupt_sibling_without_deleting() {
        let tmp = TempDir::new("flts_load_corrupt");
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
        let corrupt_path = deck.join("poder_verb.sync-conflict-corrupt.json");
        tokio::fs::write(&corrupt_path, b"{not valid json")
            .await
            .unwrap();

        let loaded = store
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(loaded.translations, vec!["мочь"]);
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
                &card_with("poder", "verb", vec!["мочь"], vec![example(book, 0, 0, "a", "1")]),
                "spa",
                "rus",
            )
            .await
            .unwrap();
        let deck = tmp.path.join("cards").join("spa-rus");
        write_pretty(
            &deck.join("poder_verb.sync-conflict-X.json"),
            &card_with("poder", "verb", vec!["уметь"], vec![example(book, 1, 1, "b", "2")]),
        )
        .await;

        store.load("spa", "rus", "poder", "verb").await.unwrap();

        let entries = deck_entries(&deck);
        assert!(
            entries.iter().all(|n| !n.contains('~')),
            "found stray temp file in {entries:?}"
        );
        assert_eq!(entries, vec!["poder_verb.json"]);
    }

    #[tokio::test]
    async fn load_returns_none_when_canonical_absent_even_with_siblings() {
        let tmp = TempDir::new("flts_load_canonical_absent");
        let store = LibraryCardStore::new(&tmp.path);
        let book = Uuid::new_v4();
        // Bootstrap the deck dir via a save we then remove.
        store
            .save(
                &card_with("poder", "verb", vec!["мочь"], vec![example(book, 0, 0, "a", "1")]),
                "spa",
                "rus",
            )
            .await
            .unwrap();
        let deck = tmp.path.join("cards").join("spa-rus");
        let canonical = deck.join("poder_verb.json");
        let conflict_path = deck.join("poder_verb.sync-conflict-X.json");
        write_pretty(
            &conflict_path,
            &card_with("poder", "verb", vec!["уметь"], vec![example(book, 1, 1, "b", "2")]),
        )
        .await;
        tokio::fs::remove_file(&canonical).await.unwrap();

        let loaded = store.load("spa", "rus", "poder", "verb").await.unwrap();
        assert!(loaded.is_none(), "expected None when canonical is absent");
        assert!(conflict_path.exists(), "sibling must be untouched when canonical is absent");
    }

    #[tokio::test]
    async fn list_pairs_returns_empty_when_root_missing() {
        let tmp = TempDir::new("flts_list_pairs_empty");
        let store = LibraryCardStore::new(&tmp.path);
        let pairs = store.list_pairs().await.unwrap();
        assert!(pairs.is_empty(), "expected empty list when cards dir is missing");
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
}
