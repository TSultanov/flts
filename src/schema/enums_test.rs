use super::*;
use crate::error::Error;
use crate::test_utils::{TempDir, committed};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
enum Shelf {
    Fiction = 0,
    Reference = 1,
    Rare = 2,
}

impl UnitEnum for Shelf {
    const NAME: &'static str = "shelf";

    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(code: u8) -> Result<Self> {
        match code {
            0 => Ok(Self::Fiction),
            1 => Ok(Self::Reference),
            2 => Ok(Self::Rare),
            other => Err(Error::unsupported(format!("unknown shelf {other}"))),
        }
    }
}

fn row(index: usize, rows: usize) -> Row {
    Row::of(index, rows, "book").unwrap()
}

#[test]
fn every_variant_round_trips_through_its_discriminant() {
    for variant in [Shelf::Fiction, Shelf::Reference, Shelf::Rare] {
        assert_eq!(Shelf::from_u8(variant.to_u8()).unwrap(), variant);
    }
}

#[test]
fn an_enum_column_reads_back_the_variant_it_was_given() {
    let mut shelf = EnumColumn::<Shelf>::default();
    shelf.push(Shelf::Rare).unwrap();
    shelf.push(Shelf::Fiction).unwrap();

    assert_eq!(shelf.rows(), 2);
    assert_eq!(shelf.get(row(0, 2)).unwrap(), Shelf::Rare);
    assert_eq!(shelf.get(row(1, 2)).unwrap(), Shelf::Fiction);

    shelf.set(row(1, 2), Shelf::Reference).unwrap();
    assert_eq!(shelf.get(row(1, 2)).unwrap(), Shelf::Reference);
}

#[test]
fn a_discriminant_that_names_no_variant_is_reported_by_the_accessor() {
    let dir = TempDir::new("enum");
    let path = dir.join("shelves.dbs");

    let mut codes = Arena::<u8>::with_chunk_rows(4);
    codes.extend_from_slice(&[0, 99]).unwrap();
    let file = committed(&path, |updater| updater.column("book.shelf", &codes));

    let mut shelf = EnumColumn::<Shelf>::default();
    shelf.read_from("book", "shelf", &file).unwrap();

    assert_eq!(shelf.get(row(0, 2)).unwrap(), Shelf::Fiction);
    let err = shelf.get(row(1, 2)).unwrap_err();
    assert!(err.to_string().contains("unknown shelf 99"), "{err}");
    assert!(
        err.is_unsupported(),
        "Record::decode stays infallible; the variant is checked here instead"
    );
}

#[test]
fn an_enum_column_asks_for_one_byte_wide_column() {
    let mut specs = Vec::new();
    EnumColumn::<Shelf>::default().describe("book", "shelf", &mut specs);

    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "book.shelf");
    assert_eq!(specs[0].elem_size, 1);
    assert_eq!(specs[0].kind, ColumnKind::Enum { name: "shelf" });
}

#[test]
fn an_enum_column_survives_a_commit_and_a_reopen() {
    let dir = TempDir::new("enum_roundtrip");
    let path = dir.join("shelves.dbs");

    let mut shelf = EnumColumn::<Shelf>::default();
    shelf.push(Shelf::Reference).unwrap();
    shelf.push(Shelf::Rare).unwrap();
    let file = committed(&path, |updater| shelf.offer_to("book", "shelf", updater));

    let mut read = EnumColumn::<Shelf>::default();
    read.read_from("book", "shelf", &file).unwrap();

    assert_eq!(read.rows(), 2);
    assert_eq!(read.get(row(0, 2)).unwrap(), Shelf::Reference);
    assert_eq!(read.get(row(1, 2)).unwrap(), Shelf::Rare);
}
