//! Children grouped by parent, built from one scan of a reference column.

use std::ops::Range;

use super::column::{Column, Row};
use super::entity::Entity;
use super::identity::IdColumn;
use crate::error::{Error, Result};
use crate::id::Id;
use crate::storage::Arena;

pub struct ChildIndex<C: Entity> {
    starts: Vec<usize>,
    children: Vec<Id<C>>,
    rows: Vec<Row>,
    orphans: Vec<Id<C>>,
    parent_rows: usize,
}

impl<C: Entity> Clone for ChildIndex<C> {
    fn clone(&self) -> Self {
        Self {
            starts: self.starts.clone(),
            children: self.children.clone(),
            rows: self.rows.clone(),
            orphans: self.orphans.clone(),
            parent_rows: self.parent_rows,
        }
    }
}

impl<C: Entity> ChildIndex<C> {
    pub fn build<P: Entity>(
        refs: &Arena<Id<P>>,
        mine: &IdColumn<C>,
        parents: &IdColumn<P>,
        column: &str,
    ) -> Result<Self> {
        let mut buckets = Vec::with_capacity(refs.len());
        for child in 0..refs.len() {
            buckets.push(bucket(*refs.row(child)?, parents, column, child)?);
        }
        Self::group(&buckets, mine, Column::rows(parents))
    }

    pub fn build_optional<P: Entity>(
        refs: &Arena<Option<Id<P>>>,
        mine: &IdColumn<C>,
        parents: &IdColumn<P>,
        column: &str,
    ) -> Result<Self> {
        let mut buckets = Vec::with_capacity(refs.len());
        for child in 0..refs.len() {
            buckets.push(match *refs.row(child)? {
                Some(parent) => bucket(parent, parents, column, child)?,
                None => None,
            });
        }
        Self::group(&buckets, mine, Column::rows(parents))
    }

    fn group(buckets: &[Option<usize>], mine: &IdColumn<C>, parent_rows: usize) -> Result<Self> {
        let mut counts = vec![0usize; parent_rows];
        let mut orphans = Vec::new();
        for (child, bucket) in buckets.iter().enumerate() {
            match bucket {
                Some(parent) => counts[*parent] += 1,
                None => orphans.push(mine.at(Row::trusted(child))?),
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
        let mut children = vec![Id::NIL; running];
        let mut rows = vec![Row::trusted(0); running];
        for (child, bucket) in buckets.iter().enumerate() {
            if let Some(parent) = bucket {
                children[at[*parent]] = mine.at(Row::trusted(child))?;
                rows[at[*parent]] = Row::trusted(child);
                at[*parent] += 1;
            }
        }

        Ok(Self {
            starts,
            children,
            rows,
            orphans,
            parent_rows,
        })
    }

    pub fn children_of<P: Entity>(&self, parents: &IdColumn<P>, parent: Id<P>) -> &[Id<C>] {
        match self.span(parents, parent) {
            Some(span) => &self.children[span],
            None => &[],
        }
    }

    pub fn child_rows_of<P: Entity>(&self, parents: &IdColumn<P>, parent: Id<P>) -> &[Row] {
        match self.span(parents, parent) {
            Some(span) => &self.rows[span],
            None => &[],
        }
    }

    fn span<P: Entity>(&self, parents: &IdColumn<P>, parent: Id<P>) -> Option<Range<usize>> {
        let row = parents.find(parent).ok()??.index();
        if row >= self.parent_rows {
            return None;
        }
        Some(self.starts[row]..self.starts[row + 1])
    }

    pub fn orphans(&self) -> &[Id<C>] {
        &self.orphans
    }

    pub fn parent_rows(&self) -> usize {
        self.parent_rows
    }
}

fn bucket<P: Entity>(
    parent: Id<P>,
    parents: &IdColumn<P>,
    column: &str,
    child: usize,
) -> Result<Option<usize>> {
    if parent.is_nil() {
        return Ok(None);
    }
    match parents.find(parent)? {
        Some(row) => Ok(Some(row.index())),
        None => Err(Error::corrupt(
            "reference",
            format!(
                "{column} row {child} names parent {parent}, which is not a row of {}",
                P::NAME
            ),
        )),
    }
}

#[cfg(test)]
#[path = "child_index_test.rs"]
mod child_index_test;
