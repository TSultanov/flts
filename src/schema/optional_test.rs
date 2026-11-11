use super::*;
use crate::error::Error;
use crate::test_utils::library::{Book, Chapter};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Binding {
    Cloth,
    Paper,
}

impl UnitEnum for Binding {
    const NAME: &'static str = "binding";
    const VARIANTS: &'static [(&'static str, u8)] = &[("Cloth", 0), ("Paper", 1)];

    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(code: u8) -> Result<Self> {
        match code {
            0 => Ok(Self::Cloth),
            1 => Ok(Self::Paper),
            other => Err(Error::unsupported(format!("unknown binding {other}"))),
        }
    }
}

fn row(index: usize, rows: usize) -> Row {
    Row::of(index, rows, "chapter").unwrap()
}

#[test]
fn a_child_with_no_parent_reads_none_and_groups_under_nobody() {
    let book = Id::<Book>::mint();
    let mut parents = OptionalRefColumn::<Book, Chapter>::named("chapter.book");
    parents.push(Some(book)).unwrap();
    parents.push(None).unwrap();

    assert_eq!(parents.get(row(0, 2)).unwrap(), Some(book));
    assert_eq!(parents.get(row(1, 2)).unwrap(), None);
    assert_eq!(parents.references().child_rows_of(book).unwrap().len(), 1);
}

#[test]
fn a_parent_can_be_given_and_taken_away_again() {
    let book = Id::<Book>::mint();
    let mut parents = OptionalRefColumn::<Book, Chapter>::named("chapter.book");
    parents.push(None).unwrap();

    parents.set(row(0, 1), Some(book)).unwrap();
    assert_eq!(parents.references().child_rows_of(book).unwrap().len(), 1);

    parents.set(row(0, 1), None).unwrap();
    assert_eq!(parents.get(row(0, 1)).unwrap(), None);
    assert!(parents.references().child_rows_of(book).unwrap().is_empty());
    parents.audit().unwrap();
}

#[test]
fn an_optional_parent_says_so_in_its_schema() {
    let parents = OptionalRefColumn::<Book, Chapter>::named("chapter.book");
    let mut specs = Vec::new();
    parents.describe("chapter", "book", &mut specs);

    assert_eq!(
        specs[0].kind,
        ColumnKind::Nullable(Box::new(ColumnKind::Ref { parent: "book" }))
    );
    assert_eq!(parents.schema_type(), "optional reference book");
}

#[test]
fn an_absent_enum_is_not_its_first_variant() {
    let mut binding = OptionalEnumColumn::<Binding>::default();
    binding.push(None).unwrap();
    binding.push(Some(Binding::Cloth)).unwrap();
    binding.push(Some(Binding::Paper)).unwrap();

    assert_eq!(binding.get(row(0, 3)).unwrap(), None);
    assert_eq!(binding.get(row(1, 3)).unwrap(), Some(Binding::Cloth));
    assert_eq!(binding.get(row(2, 3)).unwrap(), Some(Binding::Paper));

    binding.set(row(2, 3), None).unwrap();
    assert_eq!(binding.get(row(2, 3)).unwrap(), None);
}

#[test]
fn a_carried_enum_value_must_name_a_variant() {
    let mut binding = OptionalEnumColumn::<Binding>::default();
    binding.push(Some(Binding::Paper)).unwrap();

    let mut theirs = OptionalEnumColumn::<Binding>::default();
    theirs
        .append_value_bytes(&binding.value_bytes(row(0, 1)).unwrap())
        .unwrap();
    theirs.append_value_bytes(&[0]).unwrap();

    assert_eq!(theirs.get(row(0, 2)).unwrap(), Some(Binding::Paper));
    assert_eq!(theirs.get(row(1, 2)).unwrap(), None);
    assert!(theirs.append_value_bytes(&[1, 9]).is_err());
}
