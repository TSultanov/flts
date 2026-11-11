//! A hash index over a column: bucket heads and chained links, written to the file beside its keys.

use super::column::{Column, ColumnKind, ColumnSpec, Residency};
use super::names::{bucket_name, link_name};
use crate::error::{Error, Result};
use crate::id::Id;
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater, fnv1a};

const NO_ROW: u64 = u64::MAX;
const MIN_CAPACITY: usize = 8;

pub trait IndexKey: Record + Eq {
    fn raw(self) -> u128;
}

impl<T: ?Sized + 'static> IndexKey for Id<T> {
    fn raw(self) -> u128 {
        self.uuid().to_u128()
    }
}

macro_rules! whole_numbers_key_by_their_value {
    ($($number:ty),*) => {
        $(
            impl IndexKey for $number {
                fn raw(self) -> u128 {
                    self as u128
                }
            }
        )*
    };
}

whole_numbers_key_by_their_value!(u8, u16, u32, u64, i8, i16, i32, i64);

impl IndexKey for bool {
    fn raw(self) -> u128 {
        u128::from(self)
    }
}

pub trait Keys: Column {
    type Key: IndexKey;

    fn key_at(&self, row: usize) -> Result<Self::Key>;
}

impl<K: IndexKey> Keys for Arena<K> {
    type Key = K;

    fn key_at(&self, row: usize) -> Result<K> {
        match self.get(row)? {
            Some(key) => Ok(*key),
            None => Err(Error::invalid(format!(
                "a column of {} keys has no row {row}",
                self.len()
            ))),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Link<K> {
    key: K,
    next: u64,
}

impl<K: IndexKey> Record for Link<K> {
    const SIZE: usize = K::SIZE + size_of::<u64>();

    fn encode(&self, out: &mut [u8]) {
        self.key.encode(&mut out[..K::SIZE]);
        out[K::SIZE..].copy_from_slice(&self.next.to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        let mut buf = [0u8; size_of::<u64>()];
        buf.copy_from_slice(&src[K::SIZE..]);
        Self {
            key: K::decode(&src[..K::SIZE]),
            next: u64::from_le_bytes(buf),
        }
    }
}

pub const fn capacity_for(rows: usize) -> usize {
    if rows == 0 {
        return 0;
    }
    if rows <= MIN_CAPACITY {
        return MIN_CAPACITY;
    }
    rows.next_power_of_two()
}

pub fn home_of(raw: u128, capacity: usize) -> usize {
    fnv1a(&raw.to_le_bytes()) as usize & (capacity - 1)
}

fn linked<K: IndexKey>(keys: &[K], capacity: usize) -> (Vec<u64>, Vec<Link<K>>) {
    let mut buckets = vec![NO_ROW; capacity];
    let mut links = Vec::with_capacity(keys.len());

    for (row, key) in keys.iter().enumerate() {
        let home = home_of(key.raw(), capacity);
        links.push(Link {
            key: *key,
            next: buckets[home],
        });
        buckets[home] = row as u64;
    }
    (buckets, links)
}

pub struct HashIndex<K> {
    column: String,
    buckets: Arena<u64>,
    links: Arena<Link<K>>,
}

impl<K: IndexKey> Clone for HashIndex<K> {
    fn clone(&self) -> Self {
        Self {
            column: self.column.clone(),
            buckets: self.buckets.clone(),
            links: self.links.clone(),
        }
    }
}

impl<K: IndexKey> HashIndex<K> {
    pub fn new(column: String) -> Self {
        Self {
            column,
            buckets: Arena::new(),
            links: Arena::new(),
        }
    }

    pub fn rows(&self) -> usize {
        self.links.len()
    }

    pub fn capacity(&self) -> usize {
        self.buckets.len()
    }

    pub fn agrees_with(&self, rows: usize) -> bool {
        self.links.len() == rows && self.buckets.len() == capacity_for(rows)
    }

    pub fn row_of(&self, key: K) -> Result<Option<usize>> {
        self.row_where(key, |_, _| Ok(true))
    }

    pub fn rows_of(&self, key: K) -> Result<Vec<usize>> {
        self.rows_where(key, |_, _| Ok(true))
    }

    pub fn row_where(
        &self,
        key: K,
        mut holds: impl FnMut(usize, K) -> Result<bool>,
    ) -> Result<Option<usize>> {
        let mut found = None;
        self.walk(key, |row, held| match holds(row, held)? {
            true => {
                found = Some(row);
                Ok(false)
            }
            false => Ok(true),
        })?;
        Ok(found)
    }

    pub fn rows_where(
        &self,
        key: K,
        mut holds: impl FnMut(usize, K) -> Result<bool>,
    ) -> Result<Vec<usize>> {
        let mut rows = Vec::new();
        self.walk(key, |row, held| {
            if holds(row, held)? {
                rows.push(row);
            }
            Ok(true)
        })?;
        rows.sort_unstable();
        Ok(rows)
    }

    pub fn append(&mut self, key: K) -> Result<usize> {
        self.reserve(self.links.len() + 1)?;
        let home = self.home_for(key)?;
        let head = self.head(home)?;
        let row = self.links.push(Link { key, next: head })?;
        self.buckets.set(home, row as u64)?;
        Ok(row)
    }

    pub fn repoint_row(&mut self, row: usize, now: K) -> Result<()> {
        let was = self.links.row(row)?.key;
        let home = self.home_for(now)?;
        self.reachable(home)?;

        self.unlink(row, was)?;
        let head = self.head(home)?;
        self.links.set(
            row,
            Link {
                key: now,
                next: head,
            },
        )?;
        self.buckets.set(home, row as u64)
    }

    pub fn reach_for(&self, key: K) -> Result<()> {
        match self.buckets.is_empty() {
            true => Ok(()),
            false => self.reachable(home_of(key.raw(), self.buckets.len())),
        }
    }

    pub fn make_room(&self, rows: usize) -> Result<()> {
        if capacity_for(rows) > self.buckets.len() {
            for chunk in 0..self.links.chunk_count() {
                self.links.chunk(chunk)?;
            }
        }
        self.touch_tail()
    }

    pub fn reserve(&mut self, rows: usize) -> Result<()> {
        let capacity = capacity_for(rows);
        if capacity <= self.buckets.len() {
            return Ok(());
        }

        let mut keys = Vec::with_capacity(self.links.len());
        for row in 0..self.links.len() {
            keys.push(self.links.row(row)?.key);
        }
        self.install(&keys, capacity)
    }

    pub fn rebuild(&mut self, keys: &impl Keys<Key = K>) -> Result<()> {
        let mut gathered = Vec::with_capacity(Column::rows(keys));
        for row in 0..Column::rows(keys) {
            gathered.push(keys.key_at(row)?);
        }
        self.install(&gathered, capacity_for(gathered.len()))
    }

    pub fn audit(&self, keys: &impl Keys<Key = K>) -> Result<()> {
        if self.links.len() != Column::rows(keys) {
            return Err(self.damaged(format!(
                "holds {} links for {} rows",
                self.links.len(),
                Column::rows(keys)
            )));
        }
        if self.buckets.len() != capacity_for(Column::rows(keys)) {
            return Err(self.damaged(format!(
                "holds {} buckets for {} rows",
                self.buckets.len(),
                Column::rows(keys)
            )));
        }

        let mut found = vec![false; self.links.len()];
        for home in 0..self.buckets.len() {
            let mut at = *self.buckets.row(home)?;
            while at != NO_ROW {
                let row = self.holding(at)?;
                if found[row] {
                    return Err(self.damaged(format!("reaches row {row} twice")));
                }
                found[row] = true;

                let link = *self.links.row(row)?;
                let key = keys.key_at(row)?;
                if link.key != key {
                    return Err(self.damaged(format!(
                        "files row {row} under a key that row does not hold"
                    )));
                }
                if home_of(key.raw(), self.buckets.len()) != home {
                    return Err(self.damaged(format!(
                        "hangs row {row} from bucket {home}, which is not the bucket its key names"
                    )));
                }
                at = link.next;
            }
        }
        match found.iter().position(|reached| !reached) {
            Some(row) => Err(self.damaged(format!("reaches no link for row {row}"))),
            None => Ok(()),
        }
    }

    pub fn touch_tail(&self) -> Result<()> {
        tail(&self.buckets)?;
        tail(&self.links)
    }

    pub fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: bucket_name(entity, field),
            elem_size: <u64 as Record>::SIZE,
            kind: ColumnKind::Bucket,
            counts_rows: false,
        });
        out.push(ColumnSpec {
            name: link_name(entity, field),
            elem_size: <Link<K> as Record>::SIZE,
            kind: ColumnKind::Link,
            counts_rows: false,
        });
    }

    pub fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        out.push(Residency {
            name: bucket_name(entity, field),
            resident: self.buckets.resident_chunks(),
            chunks: self.buckets.chunk_count(),
        });
        out.push(Residency {
            name: link_name(entity, field),
            resident: self.links.resident_chunks(),
            chunks: self.links.chunk_count(),
        });
    }

    pub fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.buckets = as_written(file, &bucket_name(entity, field))?;
        self.links = as_written(file, &link_name(entity, field))?;
        Ok(())
    }

    pub fn offer_to(
        &self,
        entity: &str,
        field: &str,
        updater: &mut ColumnFileUpdater,
    ) -> Result<()> {
        updater.column(&bucket_name(entity, field), &self.buckets)?;
        updater.column(&link_name(entity, field), &self.links)
    }

    pub fn rebase(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        file.rebase(&bucket_name(entity, field), &mut self.buckets)?;
        file.rebase(&link_name(entity, field), &mut self.links)
    }

    fn walk(&self, key: K, mut visit: impl FnMut(usize, K) -> Result<bool>) -> Result<()> {
        if self.buckets.is_empty() {
            return Ok(());
        }

        let home = self.home_for(key)?;
        let mut at = self.head(home)?;
        for _ in 0..self.links.len() {
            if at == NO_ROW {
                return Ok(());
            }
            let row = self.holding(at)?;
            let link = *self.links.row(row)?;
            if link.key == key && !visit(row, link.key)? {
                return Ok(());
            }
            at = link.next;
        }
        match at {
            NO_ROW => Ok(()),
            _ => Err(self.damaged(format!("chains from bucket {home} without end"))),
        }
    }

    fn unlink(&mut self, row: usize, was: K) -> Result<()> {
        let home = self.home_for(was)?;
        let skip = self.links.row(row)?.next;

        if self.head(home)? == row as u64 {
            self.buckets.set(home, skip)?;
            return Ok(());
        }

        let mut at = self.head(home)?;
        for _ in 0..self.links.len() {
            if at == NO_ROW {
                break;
            }
            let held = self.holding(at)?;
            let mut link = *self.links.row(held)?;
            if link.next == row as u64 {
                link.next = skip;
                return self.links.set(held, link);
            }
            at = link.next;
        }
        Err(self.damaged(format!(
            "hangs row {row} from no bucket the key it held names"
        )))
    }

    fn install(&mut self, keys: &[K], capacity: usize) -> Result<()> {
        let (buckets, links) = linked(keys, capacity);

        let mut laid = Arena::new();
        laid.extend_from_slice(&buckets)?;
        let mut chained = Arena::new();
        chained.extend_from_slice(&links)?;

        self.buckets = laid;
        self.links = chained;
        Ok(())
    }

    fn home_for(&self, key: K) -> Result<usize> {
        match self.buckets.is_empty() {
            true => Err(self.damaged("has no bucket to name".to_owned())),
            false => Ok(home_of(key.raw(), self.buckets.len())),
        }
    }

    fn reachable(&self, home: usize) -> Result<()> {
        self.head(home).map(|_| ())
    }

    fn head(&self, home: usize) -> Result<u64> {
        match self.buckets.get(home)? {
            Some(head) => Ok(*head),
            None => Err(self.damaged(format!("has no bucket {home}"))),
        }
    }

    fn holding(&self, at: u64) -> Result<usize> {
        let row = at as usize;
        match row < self.links.len() {
            true => Ok(row),
            false => Err(self.damaged(format!("names row {row}, which it does not hold"))),
        }
    }

    fn damaged(&self, detail: String) -> Error {
        Error::corrupt("index", format!("{} {detail}", self.column))
    }
}

fn as_written<T: Record>(file: &ColumnFile, name: &str) -> Result<Arena<T>> {
    match file.column_meta(name) {
        Some(held) if held.elem_size as usize == T::SIZE => file.column(name),
        _ => Ok(Arena::new()),
    }
}

fn tail<T: Record>(arena: &Arena<T>) -> Result<()> {
    match arena.chunk_count().checked_sub(1) {
        Some(last) => arena.chunk(last).map(|_| ()),
        None => Ok(()),
    }
}

#[cfg(test)]
#[path = "index_test.rs"]
mod index_test;
