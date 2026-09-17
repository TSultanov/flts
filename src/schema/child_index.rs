//! Children grouped by parent, built from one scan of a reference column.

use crate::error::{Error, Result};
use crate::id::Id;
use crate::storage::Arena;

pub struct ChildIndex<C: ?Sized + 'static> {
    starts: Vec<usize>,
    children: Vec<Id<C>>,
    orphans: Vec<Id<C>>,
    parent_rows: usize,
}

impl<C: ?Sized + 'static> Clone for ChildIndex<C> {
    fn clone(&self) -> Self {
        Self {
            starts: self.starts.clone(),
            children: self.children.clone(),
            orphans: self.orphans.clone(),
            parent_rows: self.parent_rows,
        }
    }
}

impl<C: ?Sized + 'static> ChildIndex<C> {
    pub fn build<P: ?Sized + 'static>(
        parents: &Arena<Id<P>>,
        parent_rows: usize,
        column: &str,
    ) -> Result<Self> {
        let mut buckets = Vec::with_capacity(parents.len());
        for child in 0..parents.len() {
            buckets.push(bucket(*parents.row(child)?, parent_rows, column, child)?);
        }
        Ok(Self::group(&buckets, parent_rows))
    }

    pub fn build_optional<P: ?Sized + 'static>(
        parents: &Arena<Option<Id<P>>>,
        parent_rows: usize,
        column: &str,
    ) -> Result<Self> {
        let mut buckets = Vec::with_capacity(parents.len());
        for child in 0..parents.len() {
            buckets.push(match *parents.row(child)? {
                Some(parent) => bucket(parent, parent_rows, column, child)?,
                None => None,
            });
        }
        Ok(Self::group(&buckets, parent_rows))
    }

    fn group(buckets: &[Option<usize>], parent_rows: usize) -> Self {
        let mut counts = vec![0usize; parent_rows];
        let mut orphans = Vec::new();
        for (child, bucket) in buckets.iter().enumerate() {
            match bucket {
                Some(parent) => counts[*parent] += 1,
                None => orphans.push(Id::from_index(child)),
            }
        }

        let mut starts = Vec::with_capacity(parent_rows + 1);
        let mut running = 0;
        for count in &counts {
            starts.push(running);
            running += count;
        }
        starts.push(running);

        let mut at = starts.clone();
        let mut children = vec![Id::from_index(0); running];
        for (child, bucket) in buckets.iter().enumerate() {
            if let Some(parent) = bucket {
                children[at[*parent]] = Id::from_index(child);
                at[*parent] += 1;
            }
        }

        Self {
            starts,
            children,
            orphans,
            parent_rows,
        }
    }

    pub fn children_of<P: ?Sized + 'static>(&self, parent: Id<P>) -> &[Id<C>] {
        let index = parent.index();
        if index >= self.parent_rows {
            return &[];
        }
        &self.children[self.starts[index]..self.starts[index + 1]]
    }

    pub fn orphans(&self) -> &[Id<C>] {
        &self.orphans
    }

    pub fn parent_rows(&self) -> usize {
        self.parent_rows
    }
}

fn bucket<P: ?Sized + 'static>(
    parent: Id<P>,
    parent_rows: usize,
    column: &str,
    child: usize,
) -> Result<Option<usize>> {
    if parent.raw() == Id::<P>::NULL {
        return Ok(None);
    }
    let index = parent.index();
    if index >= parent_rows {
        return Err(Error::corrupt(
            "reference",
            format!(
                "{column} row {child} names parent row {index}, past the {parent_rows} rows there are"
            ),
        ));
    }
    Ok(Some(index))
}

#[cfg(test)]
#[path = "child_index_test.rs"]
mod child_index_test;
