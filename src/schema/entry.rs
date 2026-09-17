//! One row, held as the entity that owns it rather than as an index into a table.

use std::marker::PhantomData;
use std::ops::Range;
use std::slice;

use super::column::Row;
use super::entity::{Child, Entity, Holds, children_of};
use super::table::Table;
use crate::error::Result;
use crate::id::Id;

pub struct Entry<'a, S, E: Entity> {
    store: &'a S,
    id: Id<E>,
    row: Row,
}

impl<S, E: Entity> Clone for Entry<'_, S, E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S, E: Entity> Copy for Entry<'_, S, E> {}

impl<'a, S: Holds<E>, E: Entity> Entry<'a, S, E> {
    pub fn of(store: &'a S, id: Id<E>) -> Result<Self> {
        let row = Table::row_of(Holds::<E>::table(store), id)?;
        Ok(Self { store, id, row })
    }

    fn trusted(store: &'a S, index: usize) -> Self {
        Self {
            store,
            id: Id::from_index(index),
            row: Row::trusted(index),
        }
    }

    pub fn id(&self) -> Id<E> {
        self.id
    }

    pub fn row(&self) -> Row {
        self.row
    }

    pub fn store(&self) -> &'a S {
        self.store
    }

    pub fn table(&self) -> &'a E::Table {
        Holds::<E>::table(self.store)
    }

    pub fn at<T>(&self, id: Id<T>) -> Result<T::Row<'a, S>>
    where
        S: Holds<T>,
        T: Entity,
    {
        Ok(Entry::of(self.store, id)?.into())
    }

    pub fn children<C>(&self) -> Result<Children<'a, S, C>>
    where
        S: Holds<C>,
        C: Child<E>,
    {
        Ok(Children {
            store: self.store,
            ids: children_of(self.store, self.id)?.iter(),
        })
    }
}

pub struct EntryMut<'a, S, E: Entity> {
    store: &'a mut S,
    id: Id<E>,
    row: Row,
}

impl<'a, S: Holds<E>, E: Entity> EntryMut<'a, S, E> {
    pub fn of(store: &'a mut S, id: Id<E>) -> Result<Self> {
        let row = Table::row_of(Holds::<E>::table(store), id)?;
        Ok(Self { store, id, row })
    }

    pub fn id(&self) -> Id<E> {
        self.id
    }

    pub fn row(&self) -> Row {
        self.row
    }

    pub fn table_mut(&mut self) -> &mut E::Table {
        Holds::<E>::table_mut(self.store)
    }
}

pub struct Children<'a, S, C: Entity> {
    store: &'a S,
    ids: slice::Iter<'a, Id<C>>,
}

impl<'a, S: Holds<C> + 'a, C: Entity> Iterator for Children<'a, S, C> {
    type Item = C::Row<'a, S>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Entry::trusted(self.store, self.ids.next()?.index()).into())
    }
}

pub struct Rows<'a, S, E: Entity> {
    store: &'a S,
    rows: Range<usize>,
    marker: PhantomData<fn() -> E>,
}

impl<'a, S: Holds<E>, E: Entity> Rows<'a, S, E> {
    pub fn all(store: &'a S) -> Self {
        Self {
            store,
            rows: 0..Table::rows(Holds::<E>::table(store)),
            marker: PhantomData,
        }
    }
}

impl<'a, S: Holds<E> + 'a, E: Entity> Iterator for Rows<'a, S, E> {
    type Item = E::Row<'a, S>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Entry::trusted(self.store, self.rows.next()?).into())
    }
}

#[cfg(test)]
#[path = "entry_test.rs"]
mod entry_test;
