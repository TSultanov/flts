use std::sync::Arc;

use tokio::sync::Notify;

use crate::{
    card::{AnkiData, Card, lemma_slug},
    store::LibraryStore,
};

#[derive(Clone)]
pub struct LibraryCardStore {
    inner: Arc<CardStoreInner>,
}

struct CardStoreInner {
    store: LibraryStore,
    change_notify: Notify,
}

/// One compare-and-set of a card's retention data.
#[derive(Debug, Clone)]
pub struct AnkiUpdate {
    pub source: String,
    pub target: String,
    pub slug: String,
    /// What the data was when it was read.
    pub expected: Option<AnkiData>,
    pub data: AnkiData,
}

impl LibraryCardStore {
    pub fn new(store: LibraryStore) -> Self {
        Self {
            inner: Arc::new(CardStoreInner {
                store,
                change_notify: Notify::new(),
            }),
        }
    }

    /// Wake signal fired when `create_if_absent` adds a card; retention
    /// updates are silent.
    pub fn change_notify(&self) -> &Notify {
        &self.inner.change_notify
    }

    /// One store transaction. Once on the blocking pool it runs to the end
    /// even when the caller is cancelled; the store refuses it after closing.
    async fn transaction<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut crate::store::Inner) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        self.inner.store.run(f).await
    }

    /// Like `transaction`, under the store's read lock.
    async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&crate::store::Inner) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        self.inner.store.read(f).await
    }

    pub async fn load(
        &self,
        source: &str,
        target: &str,
        slug: &str,
    ) -> anyhow::Result<Option<Card>> {
        let (source, target, slug) = (source.to_owned(), target.to_owned(), slug.to_owned());
        self.read(move |db| db.card(&source, &target, &slug)).await
    }

    /// Every card of one pair with its slug, sorted by slug.
    pub async fn load_pair(
        &self,
        source: &str,
        target: &str,
    ) -> anyhow::Result<Vec<(String, Card)>> {
        let (source, target) = (source.to_owned(), target.to_owned());
        self.read(move |db| db.cards_in_pair(&source, &target))
            .await
    }

    /// `Ok(false)` when the card already exists; it is left as it is.
    pub async fn create_if_absent(
        &self,
        card: &Card,
        source: &str,
        target: &str,
    ) -> anyhow::Result<bool> {
        let slug = lemma_slug(&card.lemma.to_lowercase());
        let (this, card) = (self.clone(), card.clone());
        let (source, target) = (source.to_owned(), target.to_owned());
        self.transaction(move |db| {
            if db.card_anki_data(&source, &target, &slug)?.is_some() {
                // It may be staged by a create whose commit failed.
                db.publish()?;
                return Ok(false);
            }
            db.stage_card(&source, &target, &slug, &card, Some(&card.anki_data))?;
            let published = db.publish();
            this.inner.change_notify.notify_one();
            published?;
            Ok(true)
        })
        .await
    }

    /// Compare-and-set on the retention data, so a sync pass never overwrites
    /// a change it did not see.
    pub async fn update_anki_data(
        &self,
        source: &str,
        target: &str,
        slug: &str,
        expected: Option<AnkiData>,
        data: AnkiData,
    ) -> anyhow::Result<bool> {
        let update = AnkiUpdate {
            source: source.to_owned(),
            target: target.to_owned(),
            slug: slug.to_owned(),
            expected,
            data,
        };
        Ok(self.update_anki_data_batch(vec![update]).await?[0])
    }

    /// `update_anki_data` for many cards under one commit; one answer per
    /// update, in order.
    pub async fn update_anki_data_batch(
        &self,
        updates: Vec<AnkiUpdate>,
    ) -> anyhow::Result<Vec<bool>> {
        if updates.is_empty() {
            return Ok(Vec::new());
        }
        self.transaction(move |db| {
            let mut applied = Vec::with_capacity(updates.len());
            let mut failure = None;
            for update in &updates {
                let (source, target, slug) = (&update.source, &update.target, &update.slug);
                match db.set_card_anki_data(source, target, slug, &update.expected, &update.data) {
                    Ok(set) => applied.push(set),
                    Err(err) => {
                        failure = Some(err);
                        break;
                    }
                }
            }
            // What was staged before a failure is committed all the same.
            let published = db.publish();
            if let Some(err) = failure {
                return Err(err);
            }
            // Rows some other write left staged are not this call's failure.
            if applied.contains(&true) {
                published?;
            }
            Ok(applied)
        })
        .await
    }

    /// Removes the cards named as `(source, target, slug)` under one commit;
    /// the number that were there.
    pub async fn remove_batch(
        &self,
        cards: Vec<(String, String, String)>,
    ) -> anyhow::Result<usize> {
        if cards.is_empty() {
            return Ok(0);
        }
        self.transaction(move |db| {
            let mut removed = 0;
            for (source, target, slug) in &cards {
                removed += usize::from(db.remove_card(source, target, slug)?);
            }
            db.publish()?;
            Ok(removed)
        })
        .await
    }

    /// Language pairs that have cards, as `(source, target)`.
    pub async fn list_pairs(&self) -> anyhow::Result<Vec<(String, String)>> {
        let pairs = self.inner.store.read(|db| db.card_pairs()).await?;
        Ok(pairs
            .into_iter()
            .filter_map(|pair| {
                pair.split_once('-')
                    .map(|(source, target)| (source.to_owned(), target.to_owned()))
            })
            .collect())
    }

    /// Slugs of every card in one pair, sorted.
    pub async fn list_cards_in_pair(
        &self,
        source: &str,
        target: &str,
    ) -> anyhow::Result<Vec<String>> {
        let (source, target) = (source.to_owned(), target.to_owned());
        self.inner
            .store
            .read(move |db| db.card_slugs(&source, &target))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, time::Duration};

    use uuid::Uuid;

    use super::*;
    use crate::{
        card::{AnkiState, Example, card_id},
        test_utils::TempDir,
    };

    fn fixture() -> (TempDir, LibraryCardStore) {
        let dir = TempDir::new("flts_cards");
        let store = LibraryCardStore::new(LibraryStore::open(&dir.path.join("library")).unwrap());
        (dir, store)
    }

    fn example(paragraph: usize) -> Example {
        Example {
            source: format!("source {paragraph}"),
            translation: format!("translation {paragraph}"),
            book_id: Uuid::nil(),
            chapter: 0,
            paragraph,
        }
    }

    fn card(lemma: &str, translation: &str, examples: Vec<Example>) -> Card {
        Card {
            version: 2,
            id: card_id("spa", "rus", &lemma_slug(lemma)),
            lemma: lemma.into(),
            translations: BTreeMap::from([("verb".to_string(), vec![translation.to_string()])]),
            examples,
            anki_data: None,
        }
    }

    fn anki(state: AnkiState, stability: Option<f64>) -> AnkiData {
        AnkiData {
            state,
            interval_days: None,
            ease_factor: None,
            fsrs_difficulty: None,
            fsrs_stability: stability,
        }
    }

    fn with_anki(mut card: Card, data: AnkiData) -> Card {
        card.anki_data = Some(data);
        card
    }

    async fn woken(store: &LibraryCardStore) -> bool {
        tokio::time::timeout(Duration::from_millis(50), store.change_notify().notified())
            .await
            .is_ok()
    }

    async fn create(store: &LibraryCardStore, card: Card) {
        assert!(store.create_if_absent(&card, "spa", "rus").await.unwrap());
    }

    fn fail_commits(store: &LibraryCardStore, count: usize) {
        let failing = |db: &mut crate::store::Inner| {
            db.fail_commits = count;
            Ok(())
        };
        store.inner.store.run_blocking(failing).unwrap();
    }

    fn unpublished(store: &LibraryCardStore) -> bool {
        let store = &store.inner.store;
        store.run_blocking(|db| Ok(db.has_unpublished())).unwrap()
    }

    fn stored(store: &LibraryCardStore, slug: &str) -> Option<Card> {
        let store = &store.inner.store;
        store
            .run_blocking(|db| db.card("spa", "rus", slug))
            .unwrap()
    }

    /// What a closed store left on disk.
    fn on_disk(store: &LibraryCardStore, slug: &str) -> Option<Card> {
        let root = store.inner.store.shared_dir().parent().unwrap();
        LibraryStore::open(root)
            .unwrap()
            .run_blocking(|db| db.card("spa", "rus", slug))
            .unwrap()
    }

    #[tokio::test]
    async fn load_returns_none_for_a_missing_card() {
        let (_dir, store) = fixture();
        assert!(store.load("spa", "rus", "poder").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn create_if_absent_creates_once_and_leaves_the_card_alone_after() {
        let (_dir, store) = fixture();
        let first = card("poder", "мочь", vec![example(0)]);
        assert!(store.create_if_absent(&first, "spa", "rus").await.unwrap());
        let second = card("poder", "уметь", vec![example(1)]);
        assert!(!store.create_if_absent(&second, "spa", "rus").await.unwrap());

        let stored = store.load("spa", "rus", "poder").await.unwrap().unwrap();
        assert_eq!(stored, first);
    }

    #[tokio::test]
    async fn a_removed_card_is_gone_and_can_be_made_again() {
        let dir = TempDir::new("flts_cards");
        let root = dir.path.join("library");
        let old = card("poder", "мочь", vec![example(0)]);
        let new = card("poder", "мочь", vec![example(1)]);
        {
            let store = LibraryCardStore::new(LibraryStore::open(&root).unwrap());
            store.create_if_absent(&old, "spa", "rus").await.unwrap();
            create(&store, card("saber", "знать", vec![example(0)])).await;
            let named = |slug: &str| ("spa".to_owned(), "rus".to_owned(), slug.to_owned());
            assert_eq!(
                store
                    .remove_batch(vec![named("poder"), named("nada")])
                    .await
                    .unwrap(),
                1
            );
            assert_eq!(store.load("spa", "rus", "poder").await.unwrap(), None);
            assert_eq!(
                store.list_cards_in_pair("spa", "rus").await.unwrap(),
                ["saber"]
            );

            assert!(store.create_if_absent(&new, "spa", "rus").await.unwrap());
        }
        let store = LibraryCardStore::new(LibraryStore::open(&root).unwrap());
        assert_eq!(
            store.load("spa", "rus", "poder").await.unwrap(),
            Some(new),
            "the old example stays out"
        );
    }

    #[tokio::test]
    async fn a_create_whose_commit_failed_lands_on_retry() {
        let dir = TempDir::new("flts_cards");
        let root = dir.path.join("library");
        let new = card("poder", "мочь", vec![example(0)]);
        {
            let store = LibraryCardStore::new(LibraryStore::open(&root).unwrap());
            assert_eq!(store.load("spa", "rus", "poder").await.unwrap(), None);
            fail_commits(&store, 1);
            assert!(store.create_if_absent(&new, "spa", "rus").await.is_err());
            assert!(unpublished(&store));
            assert_eq!(
                store.load("spa", "rus", "poder").await.unwrap(),
                Some(new.clone()),
                "the staged card is read"
            );
            assert!(woken(&store).await, "the staged card is there to sync");

            assert!(!store.create_if_absent(&new, "spa", "rus").await.unwrap());
            assert!(!unpublished(&store), "the retry committed the card");
        }
        let store = LibraryCardStore::new(LibraryStore::open(&root).unwrap());
        assert_eq!(store.load("spa", "rus", "poder").await.unwrap(), Some(new));
    }

    #[tokio::test]
    async fn a_retry_that_still_cannot_commit_fails_too() {
        let (_dir, store) = fixture();
        let new = card("poder", "мочь", vec![]);
        fail_commits(&store, 2);
        assert!(store.create_if_absent(&new, "spa", "rus").await.is_err());
        assert!(store.create_if_absent(&new, "spa", "rus").await.is_err());
        assert!(unpublished(&store));
        assert!(!store.create_if_absent(&new, "spa", "rus").await.unwrap());
        assert!(!unpublished(&store));
    }

    #[tokio::test]
    async fn a_card_survives_a_reopen() {
        let dir = TempDir::new("flts_cards");
        let root = dir.path.join("library");
        let data = anki(AnkiState::Active, Some(12.5));
        let saved = with_anki(card("poder", "мочь", vec![example(3)]), data);
        {
            let store = LibraryCardStore::new(LibraryStore::open(&root).unwrap());
            create(&store, saved.clone()).await;
        }
        let store = LibraryCardStore::new(LibraryStore::open(&root).unwrap());
        assert_eq!(
            store.load("spa", "rus", "poder").await.unwrap(),
            Some(saved)
        );
    }

    #[tokio::test]
    async fn two_devices_union_a_card_and_agree_on_its_order() {
        let dir = TempDir::new("flts_cards");
        let a = LibraryStore::open(&dir.path.join("a/library")).unwrap();
        let b = LibraryStore::open(&dir.path.join("b/library")).unwrap();
        let (cards_a, cards_b) = (
            LibraryCardStore::new(a.clone()),
            LibraryCardStore::new(b.clone()),
        );
        create(&cards_a, card("poder", "a1", vec![example(0)])).await;
        create(&cards_b, card("poder", "b1", vec![example(1)])).await;
        crate::test_utils::copy_shards(a.shared_dir(), b.shared_dir());
        crate::test_utils::copy_shards(b.shared_dir(), a.shared_dir());
        for store in [&a, &b] {
            store.run_blocking(|db| db.refresh().map(drop)).unwrap();
        }

        let on_a = cards_a.load("spa", "rus", "poder").await.unwrap().unwrap();
        let on_b = cards_b.load("spa", "rus", "poder").await.unwrap().unwrap();
        assert_eq!(on_a, on_b);
        let mut translations = on_a.translations["verb"].clone();
        translations.sort();
        assert_eq!(translations, ["a1", "b1"]);
        assert_eq!(on_a.examples.len(), 2);
        assert_eq!(
            cards_a.list_cards_in_pair("spa", "rus").await.unwrap(),
            ["poder"]
        );
    }

    #[tokio::test]
    async fn a_card_written_on_one_device_is_removed_by_another() {
        let dir = TempDir::new("flts_cards");
        let a = LibraryStore::open(&dir.path.join("a/library")).unwrap();
        let b = LibraryStore::open(&dir.path.join("b/library")).unwrap();
        let (cards_a, cards_b) = (
            LibraryCardStore::new(a.clone()),
            LibraryCardStore::new(b.clone()),
        );
        let sync = || {
            crate::test_utils::copy_shards(a.shared_dir(), b.shared_dir());
            crate::test_utils::copy_shards(b.shared_dir(), a.shared_dir());
            for store in [&a, &b] {
                store.run_blocking(|db| db.refresh().map(drop)).unwrap();
            }
        };
        create(&cards_a, card("poder", "мочь", vec![example(0)])).await;
        create(&cards_a, card("saber", "знать", vec![example(0)])).await;
        sync();

        let poder = vec![("spa".to_owned(), "rus".to_owned(), "poder".to_owned())];
        assert_eq!(cards_b.remove_batch(poder).await.unwrap(), 1);
        sync();
        for cards in [&cards_a, &cards_b] {
            assert_eq!(cards.load("spa", "rus", "poder").await.unwrap(), None);
            assert_eq!(
                cards.list_cards_in_pair("spa", "rus").await.unwrap(),
                ["saber"]
            );
        }

        let again = card("poder", "уметь", vec![example(1)]);
        create(&cards_a, again.clone()).await;
        sync();
        for cards in [&cards_a, &cards_b] {
            assert_eq!(
                cards.load("spa", "rus", "poder").await.unwrap(),
                Some(again.clone())
            );
        }
    }

    #[tokio::test]
    async fn a_removed_card_is_written_again_after_its_history_is_pruned() {
        let dir = TempDir::new("flts_cards");
        let a = LibraryStore::open(&dir.path.join("a/library")).unwrap();
        let b = LibraryStore::open(&dir.path.join("b/library")).unwrap();
        let (cards_a, cards_b) = (
            LibraryCardStore::new(a.clone()),
            LibraryCardStore::new(b.clone()),
        );
        let sync = || {
            crate::test_utils::copy_shards(a.shared_dir(), b.shared_dir());
            crate::test_utils::copy_shards(b.shared_dir(), a.shared_dir());
            for store in [&a, &b] {
                store.run_blocking(|db| db.refresh().map(drop)).unwrap();
            }
        };
        create(&cards_a, card("poder", "мочь", vec![example(0)])).await;
        sync();
        let poder = vec![("spa".to_owned(), "rus".to_owned(), "poder".to_owned())];
        assert_eq!(cards_b.remove_batch(poder).await.unwrap(), 1);
        sync();
        sync();
        let pruned = a.run_blocking(|db| db.maintain(&[])).unwrap().pruned;
        assert!(pruned > 0, "the rows the removal wrote over are dropped");

        let again = card("poder", "мочь", vec![example(0)]);
        create(&cards_a, again.clone()).await;
        assert_eq!(
            cards_a.load("spa", "rus", "poder").await.unwrap(),
            Some(again)
        );
    }

    #[tokio::test]
    async fn update_anki_data_is_a_compare_and_set() {
        let (_dir, store) = fixture();
        let first = anki(AnkiState::Active, Some(5.0));
        let second = anki(AnkiState::Suspended, None);

        assert!(
            !store
                .update_anki_data("spa", "rus", "poder", None, first.clone())
                .await
                .unwrap(),
            "a missing card is not created"
        );
        assert!(store.load("spa", "rus", "poder").await.unwrap().is_none());

        create(&store, card("poder", "мочь", vec![example(0)])).await;
        assert!(
            store
                .update_anki_data("spa", "rus", "poder", None, first.clone())
                .await
                .unwrap()
        );
        assert!(
            !store
                .update_anki_data("spa", "rus", "poder", None, second.clone())
                .await
                .unwrap(),
            "a stale expectation must not overwrite"
        );
        let stored = store.load("spa", "rus", "poder").await.unwrap().unwrap();
        assert_eq!(stored.anki_data, Some(first.clone()));
        assert_eq!(stored.translations["verb"], ["мочь"]);
        assert_eq!(stored.examples.len(), 1);

        assert!(
            store
                .update_anki_data("spa", "rus", "poder", Some(first), second.clone())
                .await
                .unwrap()
        );
        let stored = store.load("spa", "rus", "poder").await.unwrap().unwrap();
        assert_eq!(stored.anki_data, Some(second));
    }

    #[tokio::test]
    async fn an_update_whose_commit_failed_stays_staged_and_read() {
        let (_dir, store) = fixture();
        create(&store, card("poder", "мочь", vec![])).await;

        let active = anki(AnkiState::Active, Some(90.0));
        fail_commits(&store, 1);
        assert!(
            store
                .update_anki_data("spa", "rus", "poder", None, active.clone())
                .await
                .is_err()
        );
        assert!(unpublished(&store));
        let read = store.load("spa", "rus", "poder").await.unwrap().unwrap();
        assert_eq!(read.anki_data, Some(active));
    }

    #[tokio::test]
    async fn a_pair_loads_whole_and_sorted() {
        let (_dir, store) = fixture();
        assert!(store.load_pair("spa", "rus").await.unwrap().is_empty());
        let poder = with_anki(
            card("poder", "мочь", vec![example(0)]),
            anki(AnkiState::Active, Some(5.0)),
        );
        let comer = card("comer", "есть", vec![]);
        create(&store, poder.clone()).await;
        create(&store, comer.clone()).await;
        store
            .create_if_absent(&card("house", "дом", vec![]), "eng", "rus")
            .await
            .unwrap();

        assert_eq!(
            store.load_pair("spa", "rus").await.unwrap(),
            [("comer".to_string(), comer), ("poder".to_string(), poder)]
        );
        let other = store.load_pair("eng", "rus").await.unwrap();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].1.id, card_id("eng", "rus", "house"));
    }

    #[tokio::test]
    async fn a_batch_is_a_compare_and_set_per_card() {
        let (_dir, store) = fixture();
        let held = anki(AnkiState::Suspended, None);
        create(&store, card("poder", "мочь", vec![example(0)])).await;
        create(
            &store,
            with_anki(card("comer", "есть", vec![]), held.clone()),
        )
        .await;
        assert!(woken(&store).await, "the creates' wake is spent here");

        let active = anki(AnkiState::Active, Some(90.0));
        let update = |slug: &str| AnkiUpdate {
            source: "spa".into(),
            target: "rus".into(),
            slug: slug.into(),
            expected: None,
            data: active.clone(),
        };
        let updates = vec![update("poder"), update("comer"), update("ausente")];
        assert_eq!(
            store.update_anki_data_batch(updates).await.unwrap(),
            [true, false, false]
        );
        assert!(!unpublished(&store));

        let poder = stored(&store, "poder").unwrap();
        assert_eq!(poder.anki_data, Some(active));
        assert_eq!(poder.examples.len(), 1, "the rest of the card is kept");
        assert_eq!(stored(&store, "comer").unwrap().anki_data, Some(held));
        assert!(stored(&store, "ausente").is_none());
        assert!(!woken(&store).await, "retention updates are silent");
    }

    #[tokio::test]
    async fn a_batch_whose_commit_failed_stays_staged_and_read() {
        let (_dir, store) = fixture();
        create(&store, card("poder", "мочь", vec![])).await;
        create(&store, card("comer", "есть", vec![])).await;
        let active = anki(AnkiState::Active, Some(90.0));
        let updates = ["poder", "comer"]
            .map(|slug| AnkiUpdate {
                source: "spa".into(),
                target: "rus".into(),
                slug: slug.into(),
                expected: None,
                data: active.clone(),
            })
            .to_vec();

        fail_commits(&store, 1);
        assert!(store.update_anki_data_batch(updates).await.is_err());
        assert!(unpublished(&store));
        for slug in ["poder", "comer"] {
            let read = store.load("spa", "rus", slug).await.unwrap().unwrap();
            assert_eq!(read.anki_data, Some(active.clone()));
        }
    }

    #[tokio::test]
    async fn pairs_and_cards_are_listed_sorted() {
        let (_dir, store) = fixture();
        assert!(store.list_pairs().await.unwrap().is_empty());
        assert!(
            store
                .list_cards_in_pair("spa", "rus")
                .await
                .unwrap()
                .is_empty()
        );

        for lemma in ["poder", "comer"] {
            create(&store, card(lemma, "x", vec![])).await;
        }
        store
            .create_if_absent(&card("house", "x", vec![]), "eng", "rus")
            .await
            .unwrap();

        assert_eq!(
            store.list_pairs().await.unwrap(),
            [
                ("eng".to_string(), "rus".to_string()),
                ("spa".to_string(), "rus".to_string())
            ]
        );
        assert_eq!(
            store.list_cards_in_pair("spa", "rus").await.unwrap(),
            ["comer", "poder"]
        );
        assert_eq!(
            store.list_cards_in_pair("eng", "rus").await.unwrap(),
            ["house"]
        );
        assert!(
            store
                .list_cards_in_pair("fra", "rus")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn only_user_writes_wake_the_sync_pass() {
        let (_dir, store) = fixture();

        assert!(
            store
                .create_if_absent(&card("comer", "есть", vec![]), "spa", "rus")
                .await
                .unwrap()
        );
        assert!(woken(&store).await, "create wakes");

        assert!(
            !store
                .create_if_absent(&card("comer", "есть", vec![]), "spa", "rus")
                .await
                .unwrap()
        );
        assert!(
            !woken(&store).await,
            "a create that found the card is silent"
        );

        assert!(
            store
                .update_anki_data("spa", "rus", "comer", None, anki(AnkiState::Active, None))
                .await
                .unwrap()
        );
        assert!(!woken(&store).await, "update_anki_data is silent");
    }

    #[tokio::test]
    async fn writes_fail_after_close() {
        let (_dir, store) = fixture();
        create(&store, card("poder", "мочь", vec![])).await;
        store.inner.store.close().await;

        let other = card("comer", "есть", vec![]);
        assert!(store.create_if_absent(&other, "spa", "rus").await.is_err());
        assert!(
            store
                .update_anki_data("spa", "rus", "poder", None, anki(AnkiState::Active, None))
                .await
                .is_err()
        );
        assert!(store.load("spa", "rus", "poder").await.is_err());
        assert!(store.load_pair("spa", "rus").await.is_err());
        assert!(
            on_disk(&store, "comer").is_none(),
            "nothing was written after the close"
        );
        assert!(on_disk(&store, "poder").unwrap().anki_data.is_none());
    }

    #[tokio::test]
    async fn a_cancelled_caller_does_not_cancel_its_accepted_write() {
        let (_dir, store) = fixture();
        // Holding the store keeps the write parked on the blocking pool.
        let blocker = store.inner.store.clone();
        let (entered, held) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let parked = tokio::task::spawn_blocking(move || {
            blocker
                .run_blocking(|_| {
                    entered.send(()).unwrap();
                    let _ = released.recv();
                    Ok(())
                })
                .unwrap()
        });
        tokio::task::spawn_blocking(move || held.recv().unwrap())
            .await
            .unwrap();

        let pending = tokio::spawn({
            let store = store.clone();
            async move {
                store
                    .create_if_absent(&card("poder", "мочь", vec![]), "spa", "rus")
                    .await
            }
        });
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        pending.abort();
        let _ = pending.await;
        let closing = tokio::spawn({
            let store = store.clone();
            async move { store.inner.store.close().await }
        });
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(!closing.is_finished(), "close waits for the accepted write");
        release.send(()).unwrap();
        parked.await.unwrap();
        closing.await.unwrap();

        assert!(
            on_disk(&store, "poder").is_some(),
            "the accepted write still landed"
        );
        assert!(
            store
                .create_if_absent(&card("comer", "есть", vec![]), "spa", "rus")
                .await
                .is_err()
        );
    }
}
