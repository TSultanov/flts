use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::anyhow;
use tokio::{
    sync::{Notify, mpsc, oneshot},
    task::JoinSet,
};

use crate::{
    book::serialization::create_random_string,
    card::{Card, card_id, familiarity_from, lemma_slug},
};

/// Entry cap before the familiarity cache is cleared wholesale. It's a derived
/// index repopulated on miss, so clearing is always safe; the cap only bounds
/// growth in a very long-lived process.
const FAM_CACHE_MAX_ENTRIES: usize = 100_000;

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
struct CardPath {
    src: String,
    tgt: String,
    slug: String,
}

impl CardPath {
    fn new(src: &str, tgt: &str, slug: &str) -> Self {
        Self {
            src: src.to_owned(),
            tgt: tgt.to_owned(),
            slug: slug.to_owned(),
        }
    }

    fn for_card(src: &str, tgt: &str, card: &Card) -> Self {
        Self::new(src, tgt, &lemma_slug(&card.lemma.to_lowercase()))
    }

    fn id(&self) -> String {
        card_id(&self.src, &self.tgt, &self.slug)
    }
}

type ModifyFn = Box<dyn FnOnce(&mut Option<Card>) + Send>;

enum CardOp {
    Load {
        reply: oneshot::Sender<anyhow::Result<Option<Card>>>,
    },
    Save {
        card: Card,
        wake: bool,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    Modify {
        f: ModifyFn,
        wake: bool,
        reply: oneshot::Sender<anyhow::Result<Option<Card>>>,
    },
}

struct Routed {
    key: CardPath,
    op: CardOp,
}

enum CacheMsg {
    Lookup {
        ids: Vec<String>,
        reply: oneshot::Sender<(u64, Vec<Option<Option<f32>>>)>,
    },
    Fill {
        generation: u64,
        entries: Vec<(String, Option<f32>)>,
    },
    Set {
        id: String,
        fam: Option<f32>,
    },
    Remove {
        id: String,
    },
}

pub struct LibraryCardStore {
    io: Arc<CardIo>,
    ops: mpsc::UnboundedSender<Routed>,
    #[cfg(test)]
    tasks: (tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>),
}

struct CardIo {
    root: PathBuf,
    cache: mpsc::UnboundedSender<CacheMsg>,
    change_notify: Arc<Notify>,
}

impl LibraryCardStore {
    pub fn new(library_root: &Path) -> Self {
        let (cache_tx, cache_rx) = mpsc::unbounded_channel();
        let (ops_tx, ops_rx) = mpsc::unbounded_channel();
        let io = Arc::new(CardIo {
            root: library_root.join("cards"),
            cache: cache_tx,
            change_notify: Arc::new(Notify::new()),
        });
        let dispatcher = tokio::spawn(dispatch_card_ops(io.clone(), ops_rx));
        let cache = tokio::spawn(serve_familiarity_cache(cache_rx));
        #[cfg(not(test))]
        drop((dispatcher, cache));
        Self {
            io,
            ops: ops_tx,
            #[cfg(test)]
            tasks: (dispatcher, cache),
        }
    }

    /// Wake signal fired by `save` and `modify`, but not their `_without_wake`
    /// variants.
    pub fn change_notify(&self) -> Arc<Notify> {
        self.io.change_notify.clone()
    }

    pub fn root(&self) -> &Path {
        &self.io.root
    }

    pub fn deck_dir(&self, source_language: &str, target_language: &str) -> PathBuf {
        self.io.deck_dir(source_language, target_language)
    }

    pub fn card_path(
        &self,
        source_language: &str,
        target_language: &str,
        lemma_slug: &str,
    ) -> PathBuf {
        self.io
            .card_path(&CardPath::new(source_language, target_language, lemma_slug))
    }

    async fn dispatch<T>(
        &self,
        key: CardPath,
        op: CardOp,
        reply: oneshot::Receiver<anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        self.ops
            .send(Routed { key, op })
            .map_err(|_| anyhow!("card store closed"))?;
        reply
            .await
            .map_err(|_| anyhow!("card store dropped reply"))?
    }

    /// The canonical card file alone — one file, no deck-dir scan, no
    /// writeback. Use [`load`] where conflict reconciliation is required.
    pub async fn load_canonical(
        &self,
        source_language: &str,
        target_language: &str,
        lemma_slug: &str,
    ) -> anyhow::Result<Option<Card>> {
        self.io
            .load_canonical(&CardPath::new(source_language, target_language, lemma_slug))
            .await
    }

    /// Load a card, merging any `.sync-conflict-*.json` siblings back into the
    /// canonical file.
    pub async fn load(
        &self,
        source_language: &str,
        target_language: &str,
        lemma_slug: &str,
    ) -> anyhow::Result<Option<Card>> {
        let (tx, rx) = oneshot::channel();
        self.dispatch(
            CardPath::new(source_language, target_language, lemma_slug),
            CardOp::Load { reply: tx },
            rx,
        )
        .await
    }

    /// Persist and wake `change_notify`. For user-driven write paths.
    pub async fn save(
        &self,
        card: &Card,
        source_language: &str,
        target_language: &str,
    ) -> anyhow::Result<()> {
        self.save_inner(card, source_language, target_language, true)
            .await
    }

    /// Persist without the wake, for paths that run inside a sync pass and
    /// must not self-trigger another.
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
        wake: bool,
    ) -> anyhow::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.dispatch(
            CardPath::for_card(source_language, target_language, card),
            CardOp::Save {
                card: card.clone(),
                wake,
                reply: tx,
            },
            rx,
        )
        .await
    }

    /// Read-modify-write on one card, written only if `f` changed it, then
    /// wake `change_notify`. Clearing the slot is not a delete.
    pub async fn modify<F>(
        &self,
        source_language: &str,
        target_language: &str,
        lemma_slug: &str,
        f: F,
    ) -> anyhow::Result<Option<Card>>
    where
        F: FnOnce(&mut Option<Card>) + Send + 'static,
    {
        self.modify_inner(
            source_language,
            target_language,
            lemma_slug,
            true,
            Box::new(f),
        )
        .await
    }

    /// [`modify`] without the wake, for writes made from inside a sync pass.
    pub async fn modify_without_wake<F>(
        &self,
        source_language: &str,
        target_language: &str,
        lemma_slug: &str,
        f: F,
    ) -> anyhow::Result<Option<Card>>
    where
        F: FnOnce(&mut Option<Card>) + Send + 'static,
    {
        self.modify_inner(
            source_language,
            target_language,
            lemma_slug,
            false,
            Box::new(f),
        )
        .await
    }

    async fn modify_inner(
        &self,
        source_language: &str,
        target_language: &str,
        lemma_slug: &str,
        wake: bool,
        f: ModifyFn,
    ) -> anyhow::Result<Option<Card>> {
        let (tx, rx) = oneshot::channel();
        self.dispatch(
            CardPath::new(source_language, target_language, lemma_slug),
            CardOp::Modify { f, wake, reply: tx },
            rx,
        )
        .await
    }

    /// Familiarity scalars for many slugs. Only renderable slugs appear; an
    /// absent slug is dormant. Cold slugs are read concurrently from their
    /// canonical file and memoized.
    pub async fn familiarities(
        &self,
        source_language: &str,
        target_language: &str,
        slugs: &[String],
    ) -> HashMap<String, f32> {
        let ids: Vec<String> = slugs
            .iter()
            .map(|slug| card_id(source_language, target_language, slug))
            .collect();
        let (tx, rx) = oneshot::channel();
        let lookup = match self.io.cache.send(CacheMsg::Lookup { ids, reply: tx }) {
            Ok(()) => rx.await.ok(),
            Err(_) => None,
        };
        let (generation, hits) = match lookup {
            Some((generation, hits)) => (Some(generation), hits),
            None => (None, vec![None; slugs.len()]),
        };

        let mut resolved: HashMap<String, Option<f32>> = HashMap::with_capacity(slugs.len());
        let mut misses: Vec<String> = Vec::new();
        for (slug, hit) in slugs.iter().zip(hits) {
            match hit {
                Some(fam) => {
                    resolved.insert(slug.clone(), fam);
                }
                None => misses.push(slug.clone()),
            }
        }

        if !misses.is_empty() {
            let loaded =
                futures_util::future::join_all(misses.into_iter().map(|slug| async move {
                    let key = CardPath::new(source_language, target_language, &slug);
                    match self.io.load_canonical(&key).await {
                        Ok(card) => {
                            let fam =
                                familiarity_from(card.as_ref().and_then(|c| c.anki_data.as_ref()));
                            (slug, fam, true)
                        }
                        // Leave a read error uncached so the next render
                        // retries from disk.
                        Err(_) => (slug, Some(0.0), false),
                    }
                }))
                .await;

            let mut entries: Vec<(String, Option<f32>)> = Vec::new();
            for (slug, fam, cacheable) in loaded {
                if cacheable {
                    entries.push((card_id(source_language, target_language, &slug), fam));
                }
                resolved.insert(slug, fam);
            }
            if let Some(generation) = generation
                && !entries.is_empty()
            {
                let _ = self.io.cache.send(CacheMsg::Fill {
                    generation,
                    entries,
                });
            }
        }

        resolved
            .into_iter()
            .filter_map(|(slug, fam)| fam.map(|f| (slug, f)))
            .collect()
    }

    /// Drops one cached familiarity, for card files changed outside our own
    /// `save` (e.g. a Syncthing delivery).
    pub fn invalidate_familiarity(
        &self,
        source_language: &str,
        target_language: &str,
        lemma_slug: &str,
    ) {
        let id = card_id(source_language, target_language, lemma_slug);
        let _ = self.io.cache.send(CacheMsg::Remove { id });
    }

    /// Sorted `<src>-<tgt>` deck directories; names lacking a `-` are skipped.
    pub async fn list_pairs(&self) -> anyhow::Result<Vec<(String, String)>> {
        let mut read_dir = match tokio::fs::read_dir(&self.io.root).await {
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

    /// Sorted lemma slugs in the pair's deck dir, skipping conflict siblings
    /// and non-`.json` files.
    pub async fn list_cards_in_pair(
        &self,
        source_language: &str,
        target_language: &str,
    ) -> anyhow::Result<Vec<String>> {
        let deck = self.io.deck_dir(source_language, target_language);
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

    #[cfg(test)]
    async fn shutdown_for_test(self) {
        let Self { io, ops, tasks } = self;
        drop(ops);
        drop(io);
        let (dispatcher, cache) = tasks;
        dispatcher.await.expect("dispatcher exits cleanly");
        cache.await.expect("familiarity cache exits cleanly");
    }
}

impl CardIo {
    fn deck_dir(&self, source_language: &str, target_language: &str) -> PathBuf {
        self.root
            .join(format!("{source_language}-{target_language}"))
    }

    fn card_path(&self, key: &CardPath) -> PathBuf {
        self.deck_dir(&key.src, &key.tgt)
            .join(format!("{}.json", key.slug))
    }

    async fn run_op(self: Arc<Self>, key: CardPath, op: CardOp) {
        match op {
            CardOp::Load { reply } => {
                let _ = reply.send(self.load_merged(&key).await);
            }
            CardOp::Save { card, wake, reply } => {
                let _ = reply.send(self.write(&key, &card, wake).await);
            }
            CardOp::Modify { f, wake, reply } => {
                let _ = reply.send(self.modify(&key, f, wake).await);
            }
        }
    }

    async fn load_canonical(&self, key: &CardPath) -> anyhow::Result<Option<Card>> {
        let canonical_path = self.card_path(key);
        if !tokio::fs::try_exists(&canonical_path).await? {
            return Ok(None);
        }
        let canonical_bytes = tokio::fs::read(&canonical_path).await?;
        Ok(Some(serde_json::from_slice(&canonical_bytes)?))
    }

    async fn load_merged(&self, key: &CardPath) -> anyhow::Result<Option<Card>> {
        let Some(mut base) = self.load_canonical(key).await? else {
            return Ok(None);
        };

        let accepted = self.scan_conflict_siblings(key).await;
        if accepted.is_empty() {
            return Ok(Some(base));
        }

        for (_, card) in &accepted {
            base.merge(card.clone());
        }

        // Normalization, not a user-driven change: don't wake sync.
        self.write(key, &base, false).await?;

        for (path, _) in accepted {
            if let Err(err) = tokio::fs::remove_file(&path).await {
                log::warn!("Failed to delete conflict sibling {path:?}: {err}");
            }
        }

        Ok(Some(base))
    }

    async fn modify(
        &self,
        key: &CardPath,
        f: ModifyFn,
        wake: bool,
    ) -> anyhow::Result<Option<Card>> {
        let loaded = self.load_merged(key).await?;
        let mut slot = loaded.clone();
        f(&mut slot);
        match (&loaded, &slot) {
            (Some(before), Some(after)) if before == after => {}
            (_, Some(after)) => self.write(key, after, wake).await?,
            (Some(_), None) => {
                log::warn!(
                    "modify cleared card {}; clearing is not a delete, file left as is",
                    key.id()
                );
            }
            (None, None) => {}
        }
        Ok(slot)
    }

    async fn write(&self, key: &CardPath, card: &Card, wake: bool) -> anyhow::Result<()> {
        let derived = lemma_slug(&card.lemma.to_lowercase());
        if derived != key.slug {
            return Err(anyhow!(
                "card lemma {:?} slugs to {derived}, expected {}",
                card.lemma,
                key.slug
            ));
        }

        let deck = self.deck_dir(&key.src, &key.tgt);
        tokio::fs::create_dir_all(&deck).await?;

        let file_name = format!("{}.json", key.slug);
        let canonical = deck.join(&file_name);
        let temp = deck.join(format!("{file_name}~{}", create_random_string(8)));

        let bytes = serde_json::to_vec_pretty(card)?;
        tokio::fs::write(&temp, bytes).await?;
        tokio::fs::rename(&temp, &canonical).await?;

        // We hold the authoritative card, so refresh without a re-read.
        let _ = self.cache.send(CacheMsg::Set {
            id: key.id(),
            fam: familiarity_from(card.anki_data.as_ref()),
        });

        if wake {
            self.change_notify.notify_one();
        }
        Ok(())
    }

    async fn scan_conflict_siblings(&self, key: &CardPath) -> Vec<(PathBuf, Card)> {
        let deck_dir = self.deck_dir(&key.src, &key.tgt);
        let canonical_file_name = format!("{}.json", key.slug);
        let file_name_prefix = format!("{}.", key.slug);
        let expected_id = key.id();

        let mut accepted: Vec<(PathBuf, Card)> = Vec::new();
        let mut read_dir = match tokio::fs::read_dir(&deck_dir).await {
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
            if !name.starts_with(&file_name_prefix) || !name.ends_with(".json") {
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
            let sibling_id = CardPath::for_card(&key.src, &key.tgt, &card).id();
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
}

fn spawn_op(
    tasks: &mut JoinSet<()>,
    task_keys: &mut HashMap<tokio::task::Id, CardPath>,
    io: &Arc<CardIo>,
    key: CardPath,
    op: CardOp,
) {
    let handle = tasks.spawn(io.clone().run_op(key.clone(), op));
    task_keys.insert(handle.id(), key);
}

async fn dispatch_card_ops(io: Arc<CardIo>, mut ops: mpsc::UnboundedReceiver<Routed>) {
    let mut inflight: HashMap<CardPath, VecDeque<CardOp>> = HashMap::new();
    let mut tasks: JoinSet<()> = JoinSet::new();
    let mut task_keys: HashMap<tokio::task::Id, CardPath> = HashMap::new();
    let mut open = true;

    loop {
        tokio::select! {
            msg = ops.recv(), if open => match msg {
                Some(Routed { key, op }) => match inflight.get_mut(&key) {
                    Some(queue) => queue.push_back(op),
                    None => {
                        inflight.insert(key.clone(), VecDeque::new());
                        spawn_op(&mut tasks, &mut task_keys, &io, key, op);
                    }
                },
                None => open = false,
            },
            Some(done) = tasks.join_next_with_id(), if !tasks.is_empty() => {
                let id = match done {
                    Ok((id, ())) => id,
                    Err(err) => {
                        log::error!("card op task failed: {err}");
                        err.id()
                    }
                };
                let Some(key) = task_keys.remove(&id) else {
                    continue;
                };
                match inflight.get_mut(&key).and_then(|queue| queue.pop_front()) {
                    Some(next) => spawn_op(&mut tasks, &mut task_keys, &io, key, next),
                    None => {
                        inflight.remove(&key);
                    }
                }
            }
        }
        if !open && tasks.is_empty() {
            break;
        }
    }
}

async fn serve_familiarity_cache(mut rx: mpsc::UnboundedReceiver<CacheMsg>) {
    let mut cache: HashMap<String, Option<f32>> = HashMap::new();
    let mut generation: u64 = 0;

    while let Some(msg) = rx.recv().await {
        match msg {
            CacheMsg::Lookup { ids, reply } => {
                let hits = ids.iter().map(|id| cache.get(id).copied()).collect();
                let _ = reply.send((generation, hits));
            }
            CacheMsg::Fill {
                generation: issued,
                entries,
            } => {
                if issued != generation {
                    continue;
                }
                if cache.len() >= FAM_CACHE_MAX_ENTRIES {
                    cache.clear();
                    generation += 1;
                    continue;
                }
                for (id, fam) in entries {
                    cache.entry(id).or_insert(fam);
                }
            }
            CacheMsg::Set { id, fam } => {
                cache.insert(id, fam);
            }
            CacheMsg::Remove { id } => {
                cache.remove(&id);
                generation += 1;
            }
        }
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
        let expected = tmp.path.join("cards").join("spa-rus").join("poder.json");
        assert!(expected.exists(), "expected card at {expected:?}");
    }

    #[tokio::test]
    async fn save_writes_pretty_json() {
        let tmp = TempDir::new("flts_card_pretty");
        let store = LibraryCardStore::new(&tmp.path);
        store.save(&sample_card(), "spa", "rus").await.unwrap();
        let path = tmp.path.join("cards").join("spa-rus").join("poder.json");
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
        // A queued permit would make notified() return immediately.
        let pending =
            tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified()).await;
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
        // A sibling whose lemma derives a different id must be skipped.
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

        let deck = tmp.path.join("cards").join("spa-rus");
        std::fs::write(deck.join("poder.sync-conflict-20260520-X.json"), b"{}").unwrap();
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
        store.save(&sample_card(), "spa", "rus").await.unwrap();
        store
            .save(
                &card_with("hello", "noun", vec!["привет"], vec![]),
                "eng",
                "rus",
            )
            .await
            .unwrap();
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

    // Stability == MATURE_DAYS collapses to exactly 1.0.
    #[tokio::test]
    async fn familiarities_maps_states_like_per_word_path() {
        let tmp = TempDir::new("flts_fam_states");
        // A fresh store starts with an empty cache, so this reads cold.
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
        let conflict_path = deck.join("poder.sync-conflict-20260520-XYZ.json");
        write_pretty(
            &conflict_path,
            &card_with(
                "poder",
                "verb",
                vec!["уметь"],
                vec![example(book, 1, 5, "b", "2")],
            ),
        )
        .await;

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

        let merged = store
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(merged.translations_flat(), vec!["мочь", "уметь"]);
        assert!(!conflict_path.exists(), "load must reconcile siblings");
    }

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn store_is_send_and_sync() {
        assert_send_sync::<LibraryCardStore>();
    }

    fn push_translation(slot: &mut Option<Card>, translation: &str) {
        slot.as_mut()
            .expect("card present")
            .translations
            .entry("verb".into())
            .or_default()
            .push(translation.into());
    }

    #[tokio::test]
    async fn modify_creates_card_when_absent() {
        let tmp = TempDir::new("flts_modify_create");
        let store = LibraryCardStore::new(&tmp.path);
        let out = store
            .modify("spa", "rus", "poder", |slot| {
                assert!(slot.is_none(), "closure must see None for an absent card");
                *slot = Some(sample_card());
            })
            .await
            .unwrap();
        assert_eq!(out, Some(sample_card()));
        let on_disk = store.load_canonical("spa", "rus", "poder").await.unwrap();
        assert_eq!(on_disk, Some(sample_card()));
    }

    #[tokio::test]
    async fn modify_applies_closure_and_persists() {
        let tmp = TempDir::new("flts_modify_apply");
        let store = LibraryCardStore::new(&tmp.path);
        store.save(&sample_card(), "spa", "rus").await.unwrap();

        let out = store
            .modify("spa", "rus", "poder", |slot| {
                push_translation(slot, "уметь")
            })
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(out.translations_flat(), vec!["мочь", "уметь"]);
        let on_disk = store
            .load_canonical("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(on_disk, out);
    }

    #[tokio::test]
    async fn modify_without_change_does_not_write_or_wake() {
        let tmp = TempDir::new("flts_modify_noop");
        let store = LibraryCardStore::new(&tmp.path);
        store
            .save_without_wake(&sample_card(), "spa", "rus")
            .await
            .unwrap();
        let path = store.card_path("spa", "rus", "poder");
        let compact = serde_json::to_vec(&sample_card()).unwrap();
        tokio::fs::write(&path, &compact).await.unwrap();

        let out = store.modify("spa", "rus", "poder", |_| {}).await.unwrap();
        assert_eq!(out, Some(sample_card()));
        assert_eq!(
            tokio::fs::read(&path).await.unwrap(),
            compact,
            "file was rewritten"
        );

        let notify = store.change_notify();
        let pending =
            tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified()).await;
        assert!(pending.is_err(), "no-op modify must not fire change_notify");
    }

    #[tokio::test]
    async fn modify_wakes_only_with_wake_flag() {
        let tmp = TempDir::new("flts_modify_wake");
        let store = LibraryCardStore::new(&tmp.path);
        store
            .save_without_wake(&sample_card(), "spa", "rus")
            .await
            .unwrap();
        let notify = store.change_notify();

        store
            .modify_without_wake("spa", "rus", "poder", |slot| push_translation(slot, "a"))
            .await
            .unwrap();
        let pending =
            tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified()).await;
        assert!(
            pending.is_err(),
            "modify_without_wake must not fire change_notify"
        );

        store
            .modify("spa", "rus", "poder", |slot| push_translation(slot, "b"))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), notify.notified())
            .await
            .expect("modify must fire change_notify");
    }

    #[tokio::test]
    async fn modify_setting_none_leaves_file() {
        let tmp = TempDir::new("flts_modify_clear");
        let store = LibraryCardStore::new(&tmp.path);
        store.save(&sample_card(), "spa", "rus").await.unwrap();

        let out = store
            .modify("spa", "rus", "poder", |slot| *slot = None)
            .await
            .unwrap();
        assert!(out.is_none());
        let on_disk = store.load_canonical("spa", "rus", "poder").await.unwrap();
        assert_eq!(on_disk, Some(sample_card()), "clearing must not delete");
    }

    #[tokio::test]
    async fn modify_rejects_card_whose_lemma_slug_mismatches_key() {
        let tmp = TempDir::new("flts_modify_mismatch");
        let store = LibraryCardStore::new(&tmp.path);
        store.save(&sample_card(), "spa", "rus").await.unwrap();

        let result = store
            .modify("spa", "rus", "poder", |slot| {
                *slot = Some(card_with("comer", "verb", vec!["есть"], vec![]));
            })
            .await;
        assert!(result.is_err(), "a card for another key must be refused");

        let deck = tmp.path.join("cards").join("spa-rus");
        assert_eq!(deck_entries(&deck), vec!["poder.json"]);
        let on_disk = store.load_canonical("spa", "rus", "poder").await.unwrap();
        assert_eq!(on_disk, Some(sample_card()));
    }

    #[tokio::test]
    async fn modify_merges_conflict_siblings_before_applying() {
        let tmp = TempDir::new("flts_modify_siblings");
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

        let out = store
            .modify("spa", "rus", "poder", |slot| {
                let seen = slot.as_ref().expect("card present").translations_flat();
                assert_eq!(seen, vec!["мочь", "уметь"], "closure must see the merge");
                push_translation(slot, "сметь");
            })
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(out.translations_flat(), vec!["мочь", "уметь", "сметь"]);
        assert_eq!(out.examples.len(), 2);
        assert!(!conflict_path.exists(), "sibling must be consumed");
        let on_disk = store
            .load_canonical("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(on_disk, out);
    }

    #[tokio::test]
    async fn save_routes_by_lemma_derived_slug() {
        let tmp = TempDir::new("flts_save_route");
        let store = LibraryCardStore::new(&tmp.path);
        let mut card = sample_card();
        card.lemma = "Poder".into();
        store.save(&card, "spa", "rus").await.unwrap();

        let deck = tmp.path.join("cards").join("spa-rus");
        assert_eq!(deck_entries(&deck), vec!["poder.json"]);
        let fam = store
            .familiarities("spa", "rus", &["poder".to_string()])
            .await;
        assert_eq!(fam.get("poder").copied(), Some(0.0));
    }

    #[tokio::test]
    async fn load_merges_sibling_with_case_preserving_lemma() {
        let tmp = TempDir::new("flts_load_case_sibling");
        let store = LibraryCardStore::new(&tmp.path);
        store.save(&sample_card(), "spa", "rus").await.unwrap();
        let deck = tmp.path.join("cards").join("spa-rus");
        let conflict_path = deck.join("poder.sync-conflict-X.json");
        let mut sibling = card_with("poder", "verb", vec!["уметь"], vec![]);
        sibling.lemma = "Poder".into();
        write_pretty(&conflict_path, &sibling).await;

        let merged = store
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(merged.translations_flat(), vec!["мочь", "уметь"]);
        assert!(
            !conflict_path.exists(),
            "case-preserving sibling must merge"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_modifies_on_same_card_are_serialized() {
        let tmp = TempDir::new("flts_modify_concurrent");
        let store = Arc::new(LibraryCardStore::new(&tmp.path));
        store.save(&sample_card(), "spa", "rus").await.unwrap();

        let mut handles = Vec::new();
        for i in 0..50 {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                store
                    .modify("spa", "rus", "poder", move |slot| {
                        push_translation(slot, &format!("t{i}"))
                    })
                    .await
                    .unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        let card = store
            .load_canonical("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert_eq!(card.translations_flat().len(), 51, "lost update");
    }

    #[tokio::test]
    async fn modifies_issued_in_order_apply_in_order() {
        let tmp = TempDir::new("flts_modify_order");
        let store = LibraryCardStore::new(&tmp.path);
        store.save(&sample_card(), "spa", "rus").await.unwrap();

        let (a, b) = tokio::join!(
            store.modify("spa", "rus", "poder", |slot| push_translation(slot, "a")),
            store.modify("spa", "rus", "poder", |slot| push_translation(slot, "b")),
        );
        a.unwrap();
        let after_b = b.unwrap().expect("card present");
        assert_eq!(after_b.translations_flat(), vec!["мочь", "a", "b"]);
    }

    #[tokio::test]
    async fn save_then_familiarities_observes_new_value() {
        let tmp = TempDir::new("flts_fam_after_save");
        let store = LibraryCardStore::new(&tmp.path);
        for i in 0..200 {
            let stability = if i % 2 == 0 { Some(90.0) } else { None };
            let card = card_with_anki("poder", AnkiState::Active, stability);
            let expected = familiarity_from(card.anki_data.as_ref()).unwrap();
            store.save_without_wake(&card, "spa", "rus").await.unwrap();
            let fam = store
                .familiarities("spa", "rus", &["poder".to_string()])
                .await;
            assert_eq!(fam.get("poder").copied(), Some(expected), "iteration {i}");
        }
    }

    async fn cache_lookup(
        tx: &mpsc::UnboundedSender<CacheMsg>,
        id: &str,
    ) -> (u64, Option<Option<f32>>) {
        let (reply, rx) = oneshot::channel();
        tx.send(CacheMsg::Lookup {
            ids: vec![id.to_owned()],
            reply,
        })
        .unwrap();
        let (generation, hits) = rx.await.unwrap();
        (generation, hits[0])
    }

    #[tokio::test]
    async fn familiarity_cache_fill_does_not_overwrite_set_entry() {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(serve_familiarity_cache(rx));
        let id = "flts_spa_rus_poder".to_owned();

        let (generation, hit) = cache_lookup(&tx, &id).await;
        assert_eq!(hit, None);
        tx.send(CacheMsg::Set {
            id: id.clone(),
            fam: Some(1.0),
        })
        .unwrap();
        tx.send(CacheMsg::Fill {
            generation,
            entries: vec![(id.clone(), Some(0.0))],
        })
        .unwrap();
        let (_, hit) = cache_lookup(&tx, &id).await;
        assert_eq!(hit, Some(Some(1.0)), "fill must not clobber a write");
    }

    #[tokio::test]
    async fn familiarity_cache_fill_after_remove_is_dropped() {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(serve_familiarity_cache(rx));
        let id = "flts_spa_rus_poder".to_owned();

        let (generation, _) = cache_lookup(&tx, &id).await;
        tx.send(CacheMsg::Remove { id: id.clone() }).unwrap();
        tx.send(CacheMsg::Fill {
            generation,
            entries: vec![(id.clone(), Some(0.5))],
        })
        .unwrap();
        let (fresh, hit) = cache_lookup(&tx, &id).await;
        assert_eq!(hit, None, "fill issued before the remove must be dropped");

        tx.send(CacheMsg::Fill {
            generation: fresh,
            entries: vec![(id.clone(), Some(0.5))],
        })
        .unwrap();
        let (_, hit) = cache_lookup(&tx, &id).await;
        assert_eq!(hit, Some(Some(0.5)), "a current fill is applied");
    }

    #[tokio::test]
    async fn store_shutdown_completes_queued_writes_and_exits() {
        let tmp = TempDir::new("flts_store_shutdown");
        let baseline = tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks();
        let store = Arc::new(LibraryCardStore::new(&tmp.path));
        store.save(&sample_card(), "spa", "rus").await.unwrap();

        let mut callers = Vec::new();
        for i in 0..20 {
            let store = store.clone();
            callers.push(tokio::spawn(async move {
                store
                    .modify("spa", "rus", "poder", move |slot| {
                        push_translation(slot, &format!("t{i}"))
                    })
                    .await
            }));
        }
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        for caller in callers {
            caller.abort();
            let _ = caller.await;
        }

        let store =
            Arc::try_unwrap(store).unwrap_or_else(|_| panic!("callers still hold the store"));
        tokio::time::timeout(std::time::Duration::from_secs(5), store.shutdown_for_test())
            .await
            .expect("store tasks must exit once the store is dropped");

        let card: Card = serde_json::from_slice(
            &tokio::fs::read(tmp.path.join("cards").join("spa-rus").join("poder.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            card.translations_flat().len(),
            21,
            "queued writes must complete"
        );
        assert_eq!(
            tokio::runtime::Handle::current()
                .metrics()
                .num_alive_tasks(),
            baseline,
            "store tasks leaked"
        );
    }
}
