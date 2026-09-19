//! One row, held as the entity that owns it rather than as an index into a table.

use std::marker::PhantomData;
use std::ops::Range;
use std::vec;

use super::column::Row;
use super::entity::{Child, Entity, Holds, across_rows_of, child_rows_of};
use super::table::Table;
use super::version::Version;
use crate::error::{Error, Result};
use crate::id::Id;

pub struct Entry<'a, S, E: Entity> {
    store: &'a S,
    row: Row,
    historical: bool,
    marker: PhantomData<fn() -> E>,
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
        Ok(Self::current_at_row(store, row))
    }

    pub fn at_row(store: &'a S, row: Row) -> Self {
        Self {
            store,
            row,
            historical: true,
            marker: PhantomData,
        }
    }

    pub fn current_at_row(store: &'a S, row: Row) -> Self {
        Self {
            store,
            row,
            historical: false,
            marker: PhantomData,
        }
    }

    pub fn is_historical(&self) -> bool {
        self.historical
    }

    pub fn sources(&self) -> Result<Vec<super::Dot>> {
        if self.historical {
            return Ok(vec![self.table().versions().raw_at(self.row)?.dot()]);
        }
        self.table().source_dots(self.id()?)
    }

    pub fn id(&self) -> Result<Id<E>> {
        Table::id_at(Holds::<E>::table(self.store), self.row)
    }

    pub fn row(&self) -> Row {
        self.row
    }

    pub fn version(&self) -> Result<Version> {
        if !self.historical && self.table().is_derived(self.id()?) {
            return Err(Error::invalid(
                "a merged row has multiple source revisions; use sources()",
            ));
        }
        let version = self.table().versions().at(self.row)?;
        Ok(if self.historical && self.table().is_derived(self.id()?) {
            version.supersede()
        } else {
            version
        })
    }

    pub fn parents(&self) -> Result<&[super::Parent]> {
        if !self.historical && self.table().is_derived(self.id()?) {
            return Err(Error::invalid(
                "a merged row has source revisions instead of a single parent list; use sources()",
            ));
        }
        self.table().versions().parents(self.row)
    }

    pub fn parent_id<P: Entity>(&self) -> Result<Id<P>>
    where
        E: Child<P>,
    {
        <E as Child<P>>::parent_at(self.table(), self.row, self.historical)
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
            rows: child_rows_of::<S, E, C>(self.store, self.id()?)?.into_iter(),
            marker: PhantomData,
        })
    }

    pub fn across<J, B>(&self) -> Result<Across<'a, S, B>>
    where
        S: Holds<J> + Holds<B>,
        B: Entity,
        J: Child<E> + Child<B>,
    {
        Ok(Across {
            store: self.store,
            rows: across_rows_of::<S, E, J, B>(self.store, self.id()?)?.into_iter(),
            marker: PhantomData,
        })
    }
}

pub struct EntryMut<'a, S, E: Entity> {
    store: &'a mut S,
    id: Id<E>,
    marker: PhantomData<fn() -> E>,
}

impl<'a, S: Holds<E>, E: Entity> EntryMut<'a, S, E> {
    pub fn hidden(store: &'a mut S, id: Id<E>) -> Result<Self> {
        Table::logical_row_of(Holds::<E>::table(store), id)?;
        Ok(Self {
            store,
            id,
            marker: PhantomData,
        })
    }

    pub fn of(store: &'a mut S, id: Id<E>) -> Result<Self> {
        Table::row_of(Holds::<E>::table(store), id)?;
        Ok(Self {
            store,
            id,
            marker: PhantomData,
        })
    }

    pub fn id(&self) -> Result<Id<E>> {
        Ok(self.id)
    }

    pub fn row(&self) -> Result<Row> {
        Table::row_of(Holds::<E>::table(self.store), self.id)
    }

    pub fn table_mut(&mut self) -> &mut E::Table {
        Holds::<E>::table_mut(self.store)
    }
}

pub struct Children<'a, S, C: Entity> {
    store: &'a S,
    rows: vec::IntoIter<Row>,
    marker: PhantomData<fn() -> C>,
}

impl<'a, S: Holds<C> + 'a, C: Entity> Iterator for Children<'a, S, C> {
    type Item = C::Row<'a, S>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Entry::current_at_row(self.store, self.rows.next()?).into())
    }
}

pub struct Across<'a, S, B: Entity> {
    store: &'a S,
    rows: vec::IntoIter<Row>,
    marker: PhantomData<fn() -> B>,
}

impl<'a, S: Holds<B> + 'a, B: Entity> Iterator for Across<'a, S, B> {
    type Item = B::Row<'a, S>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Entry::current_at_row(self.store, self.rows.next()?).into())
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
        let table = Holds::<E>::table(self.store);
        loop {
            let row = Row::trusted(self.rows.next()?);
            if table.versions().at(row).is_ok_and(Version::is_live) {
                return Some(Entry::current_at_row(self.store, row).into());
            }
        }
    }
}

#[cfg(test)]
#[path = "entry_test.rs"]
mod entry_test;
