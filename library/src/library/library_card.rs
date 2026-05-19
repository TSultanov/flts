use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::sync::Mutex;

use crate::{
    book::serialization::create_random_string,
    card::{Card, lemma_slug},
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

    pub fn card_path(&self, source_language: &str, target_language: &str, slug: &str, part_of_speech: &str) -> PathBuf {
        self.deck_dir(source_language, target_language)
            .join(format!("{slug}_{part_of_speech}.json"))
    }

    pub async fn lock_for(&self, id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        locks
            .entry(id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn load(
        &self,
        source_language: &str,
        target_language: &str,
        slug: &str,
        part_of_speech: &str,
    ) -> anyhow::Result<Option<Card>> {
        let path = self.card_path(source_language, target_language, slug, part_of_speech);
        if !tokio::fs::try_exists(&path).await? {
            return Ok(None);
        }
        let bytes = tokio::fs::read(&path).await?;
        let card: Card = serde_json::from_slice(&bytes)?;
        Ok(Some(card))
    }

    pub async fn save(&self, card: &Card, source_language: &str, target_language: &str) -> anyhow::Result<()> {
        let deck = self.deck_dir(source_language, target_language);
        tokio::fs::create_dir_all(&deck).await?;

        let slug = lemma_slug(&card.lemma);
        let part_of_speech = &card.part_of_speech;
        let file_name = format!("{slug}_{part_of_speech}.json");
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
}
