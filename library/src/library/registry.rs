use super::library_book::{BookStatus, BookToken, StopMode, owner, serve_book};
use super::{BookHandle, LibraryBook, LibraryBookMetadata, session::LibraryLease};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::{Id, JoinSet},
};
use uuid::Uuid;

type BookReply = oneshot::Sender<anyhow::Result<BookHandle>>;
type VoidReply = oneshot::Sender<anyhow::Result<()>>;
pub(super) enum BookRequest {
    Get {
        id: Uuid,
        reply: BookReply,
    },
    Cached {
        id: Uuid,
        reply: oneshot::Sender<Option<BookHandle>>,
    },
    Insert {
        id: Uuid,
        book: LibraryBook,
        reply: BookReply,
    },
    Remove {
        id: Uuid,
        reply: VoidReply,
    },
    Live {
        reply: oneshot::Sender<Vec<BookHandle>>,
    },
    Close {
        reply: VoidReply,
    },
    #[cfg(test)]
    Inflight {
        reply: oneshot::Sender<usize>,
    },
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Loading,
    Active,
    Retiring,
    Deleting,
    Deleted,
    Faulted,
}
struct Entry {
    phase: Phase,
    handle: Option<BookHandle>,
    stop: Option<watch::Sender<StopMode>>,
    waiters: Vec<BookReply>,
    deletes: Vec<VoidReply>,
}
impl Entry {
    fn loading(reply: BookReply) -> Self {
        Self {
            phase: Phase::Loading,
            handle: None,
            stop: None,
            waiters: vec![reply],
            deletes: vec![],
        }
    }
    fn reject_waiters(&mut self, error: &str) {
        for reply in self.waiters.drain(..) {
            let _ = reply.send(Err(anyhow::anyhow!("{error}")));
        }
    }
    fn stop(&mut self, mode: StopMode) {
        if let Some(handle) = &self.handle {
            *handle.admission.lock().unwrap() = match mode {
                StopMode::Delete => BookStatus::Deleting,
                StopMode::Retire => BookStatus::Retiring,
                _ => BookStatus::Closed,
            };
        }
        if let Some(stop) = &self.stop {
            stop.send_replace(mode);
        }
    }
}
enum Completed {
    Loaded(Uuid, anyhow::Result<LibraryBook>),
    Exited(Uuid, BookToken),
    Deleted(Uuid, anyhow::Result<()>),
}
#[derive(Clone, Copy)]
enum Work {
    Load(Uuid),
    Owner(Uuid),
    Delete(Uuid),
}
fn load(
    id: Uuid,
    root: &std::path::Path,
    jobs: &mut JoinSet<Completed>,
    ids: &mut HashMap<Id, Work>,
) {
    let path = root.join(id.to_string());
    let job = jobs.spawn(async move {
        Completed::Loaded(
            id,
            async {
                LibraryBook::load_from_metadata(LibraryBookMetadata::load(&path).await?).await
            }
            .await,
        )
    });
    ids.insert(job.id(), Work::Load(id));
}
fn remove(
    id: Uuid,
    root: &std::path::Path,
    jobs: &mut JoinSet<Completed>,
    ids: &mut HashMap<Id, Work>,
) {
    let path = root.join(id.to_string());
    let job = jobs.spawn(async move {
        Completed::Deleted(id, super::library_book::delete_directory(&path).await)
    });
    ids.insert(job.id(), Work::Delete(id));
}
fn start(
    entry: &mut Entry,
    mut book: LibraryBook,
    session: Uuid,
    failures: &mpsc::UnboundedSender<super::library_book::StopFailed>,
    jobs: &mut JoinSet<Completed>,
    ids: &mut HashMap<Id, Work>,
) {
    let id = book.book.id;
    let (handle, commands, publish, stop, stopped) = owner(&mut book, session);
    let admission = handle.admission.clone();
    let failures = failures.clone();
    let job = jobs.spawn(async move {
        let (id, token) = serve_book(book, commands, publish, stopped, failures, admission).await;
        Completed::Exited(id, token)
    });
    ids.insert(job.id(), Work::Owner(id));
    entry.phase = Phase::Active;
    entry.handle = Some(handle.clone());
    entry.stop = Some(stop);
    for waiter in entry.waiters.drain(..) {
        let _ = waiter.send(Ok(handle.clone()));
    }
}
fn touch(warm: &mut VecDeque<Uuid>, id: Uuid, capacity: usize) {
    warm.retain(|k| *k != id);
    if capacity != 0 {
        warm.push_back(id);
    }
    while warm.len() > capacity {
        warm.pop_front();
    }
}

pub(super) async fn serve_books(
    lease: LibraryLease,
    capacity: usize,
    mut requests: mpsc::Receiver<BookRequest>,
) {
    let root: PathBuf = lease.root.clone();
    let session = Uuid::new_v4();
    let mut entries: HashMap<Uuid, Entry> = HashMap::new();
    let mut warm = VecDeque::new();
    let mut jobs = JoinSet::new();
    let mut ids = HashMap::new();
    let (failures, mut failed) = mpsc::unbounded_channel();
    let mut sweep = tokio::time::interval(Duration::from_secs(1));
    let mut closing = false;
    let mut disconnected = false;
    let mut close_replies: Vec<VoidReply> = vec![];
    loop {
        tokio::select! {
            request = requests.recv(), if !disconnected => match request {
                None => { disconnected = true; closing = true; for entry in entries.values_mut() { if entry.phase == Phase::Active { entry.stop(StopMode::Close); } } }
                Some(BookRequest::Close {reply}) => { close_replies.push(reply); closing = true; for entry in entries.values_mut() { if entry.phase == Phase::Active { entry.stop(StopMode::Close); } } }
                Some(BookRequest::Get {id, reply}) => {
                    if closing { let _ = reply.send(Err(anyhow::anyhow!("library session closing"))); continue; }
                    match entries.get_mut(&id) {
                        Some(entry) if entry.phase == Phase::Active => {
                            if !warm.contains(&id) && Arc::strong_count(entry.handle.as_ref().unwrap()) == 1 {
                                entry.phase = Phase::Retiring; entry.waiters.push(reply); entry.stop(StopMode::Retire);
                            } else {
                                let _ = reply.send(Ok(entry.handle.as_ref().unwrap().clone())); touch(&mut warm, id, capacity);
                            }
                        }
                        Some(entry) if matches!(entry.phase, Phase::Loading | Phase::Retiring) => entry.waiters.push(reply),
                        Some(_) => { let _ = reply.send(Err(anyhow::anyhow!("book {id} deleted or unavailable"))); }
                        None => { entries.insert(id, Entry::loading(reply)); load(id, &root, &mut jobs, &mut ids); }
                    }
                }
                Some(BookRequest::Cached {id, reply}) => { let _ = reply.send(entries.get(&id).filter(|e| e.phase == Phase::Active).and_then(|e| e.handle.clone())); }
                Some(BookRequest::Insert {id, book, reply}) => {
                    if closing || entries.contains_key(&id) { let _ = reply.send(Err(anyhow::anyhow!("book already registered or library closing"))); continue; }
                    let mut entry = Entry::loading(reply);
                    start(&mut entry, book, session, &failures, &mut jobs, &mut ids);
                    entries.insert(id, entry); touch(&mut warm, id, capacity);
                }
                Some(BookRequest::Remove {id, reply}) => {
                    warm.retain(|k| *k != id);
                    if let Some(entry) = entries.get_mut(&id) {
                        entry.deletes.push(reply);
                        if entry.phase == Phase::Deleting { continue; }
                        let was_loading = entry.phase == Phase::Loading;
                        entry.phase = Phase::Deleting;
                        entry.reject_waiters("book deleted while loading");
                        if entry.handle.is_some() { entry.stop(StopMode::Delete); }
                        else if !was_loading { remove(id, &root, &mut jobs, &mut ids); }
                    } else {
                        entries.insert(id, Entry {phase: Phase::Deleting, handle: None, stop: None, waiters: vec![], deletes: vec![reply]});
                        remove(id, &root, &mut jobs, &mut ids);
                    }
                }
                Some(BookRequest::Live {reply}) => { let _ = reply.send(entries.values().filter(|e| e.phase == Phase::Active).filter_map(|e| e.handle.clone()).collect()); }
                #[cfg(test)]
                Some(BookRequest::Inflight {reply}) => { let _ = reply.send(ids.values().filter(|w| matches!(w, Work::Load(_))).count()); }
            },
            Some(failure) = failed.recv() => {
                if let Some(entry) = entries.get_mut(&failure.id) {
                    if entry.handle.as_ref().is_none_or(|h| h.token != failure.token) { continue; }
                    log::warn!("book {} retirement retained dirty state: {}", failure.id, failure.error);
                    entry.phase = Phase::Active;
                    *entry.handle.as_ref().unwrap().admission.lock().unwrap() = BookStatus::Active;
                    entry.stop.as_ref().unwrap().send_replace(StopMode::Running);
                    for waiter in entry.waiters.drain(..) { let _ = waiter.send(Ok(entry.handle.as_ref().unwrap().clone())); }
                    if closing {
                        closing = false;
                        for reply in close_replies.drain(..) { let _ = reply.send(Err(anyhow::anyhow!("flush failed: {}", failure.error))); }
                    }
                }
            }
            Some(result) = jobs.join_next_with_id(), if !jobs.is_empty() => {
                let completed = match result {
                    Ok((task, completed)) => { ids.remove(&task); completed }
                    Err(err) => {
                        let Some(work) = ids.remove(&err.id()) else { continue; };
                        let id = match work { Work::Load(id) | Work::Owner(id) | Work::Delete(id) => id };
                        if let Some(entry) = entries.get_mut(&id) {
                            entry.phase = Phase::Faulted; entry.reject_waiters(&format!("library task failed: {err}"));
                            for reply in entry.deletes.drain(..) { let _ = reply.send(Err(anyhow::anyhow!("library task failed: {err}"))); }
                        }
                        for reply in close_replies.drain(..) { let _ = reply.send(Err(anyhow::anyhow!("library task failed: {err}"))); }
                        closing = false;
                        continue;
                    }
                };
                match completed {
                    Completed::Loaded(id, loaded) => {
                        let Some(entry) = entries.get_mut(&id) else { continue; };
                        if entry.phase == Phase::Deleting { drop(loaded); remove(id, &root, &mut jobs, &mut ids); }
                        else if closing { entry.reject_waiters("library session closing"); entries.remove(&id); }
                        else { match loaded {
                            Ok(book) => { start(entry, book, session, &failures, &mut jobs, &mut ids); touch(&mut warm, id, capacity); }
                            Err(err) => { entry.reject_waiters(&format!("{err:#}")); entries.remove(&id); }
                        } }
                    }
                    Completed::Exited(id, token) => {
                        let Some(entry) = entries.get_mut(&id) else { continue; };
                        if entry.handle.as_ref().is_none_or(|h| h.token != token) { continue; }
                        entry.handle.take(); entry.stop.take();
                        if entry.phase == Phase::Deleting { remove(id, &root, &mut jobs, &mut ids); }
                        else if !closing && !entry.waiters.is_empty() { entry.phase = Phase::Loading; load(id, &root, &mut jobs, &mut ids); }
                        else { entry.reject_waiters("library session closed"); entries.remove(&id); }
                    }
                    Completed::Deleted(id, result) => {
                        let entry = entries.get_mut(&id).unwrap();
                        entry.phase = if result.is_ok() { Phase::Deleted } else { Phase::Faulted };
                        let error = result.err().map(|e| format!("{e:#}"));
                        for reply in entry.deletes.drain(..) { let _ = reply.send(match &error {None => Ok(()), Some(err) => Err(anyhow::anyhow!("{err}"))}); }
                    }
                }
            }
            _ = sweep.tick(), if !closing => {
                for (id, entry) in &mut entries {
                    if entry.phase == Phase::Active && !warm.contains(id) && entry.handle.as_ref().is_some_and(|h| Arc::strong_count(h) == 1) {
                        entry.phase = Phase::Retiring; entry.stop(StopMode::Retire);
                    }
                }
            }
        }
        if !closing {
            for (id, entry) in &mut entries {
                if entry.phase == Phase::Active
                    && !warm.contains(id)
                    && entry
                        .handle
                        .as_ref()
                        .is_some_and(|h| Arc::strong_count(h) == 1)
                {
                    entry.phase = Phase::Retiring;
                    entry.stop(StopMode::Retire);
                }
            }
        }
        if closing && jobs.is_empty() {
            for reply in close_replies.drain(..) {
                let _ = reply.send(Ok(()));
            }
            break;
        }
    }
    drop(lease);
}
