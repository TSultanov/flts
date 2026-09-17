use crate::error::Result;
use crate::schema::UnitEnum;

#[crate::field_enum]
#[derive(PartialEq, Eq, Debug)]
enum Shelf {
    Fiction,
    Reference,
    Rare,
}

#[crate::field_enum]
#[derive(PartialEq, Eq, Debug)]
enum Condition {
    Pristine = 10,
    Worn,
}

#[test]
fn a_generated_enum_numbers_its_variants_from_zero() {
    assert_eq!(Shelf::Fiction.to_u8(), 0);
    assert_eq!(Shelf::Reference.to_u8(), 1);
    assert_eq!(Shelf::Rare.to_u8(), 2);
}

#[test]
fn a_generated_enum_counts_on_from_the_discriminant_it_was_given() {
    assert_eq!(Condition::Pristine.to_u8(), 10);
    assert_eq!(Condition::Worn.to_u8(), 11);
}

#[test]
fn every_generated_variant_round_trips_through_its_discriminant() {
    for variant in [Shelf::Fiction, Shelf::Reference, Shelf::Rare] {
        assert_eq!(Shelf::from_u8(variant.to_u8()).unwrap(), variant);
    }
}

#[test]
fn a_generated_enum_refuses_a_discriminant_it_does_not_name() {
    let err = Condition::from_u8(0).unwrap_err();

    assert!(err.is_unsupported(), "{err}");
    assert!(err.to_string().contains("unknown condition 0"), "{err}");
}

#[test]
fn a_generated_enum_is_named_after_its_type_in_snake_case() {
    assert_eq!(<Shelf as UnitEnum>::NAME, "shelf");
    assert_eq!(<Condition as UnitEnum>::NAME, "condition");
}

#[test]
fn a_generated_enum_keeps_the_derives_its_author_asked_for() {
    assert_eq!(Shelf::Rare, Shelf::Rare);
    assert_ne!(Shelf::Rare, Shelf::Fiction);
    assert_eq!(format!("{:?}", Shelf::Rare), "Rare");

    let copied = Shelf::Rare;
    let _also = copied;
    assert_eq!(copied, Shelf::Rare);
}

#[test]
fn a_generated_enum_can_be_the_type_of_a_column() -> Result<()> {
    use crate::schema::{Column, EnumColumn, Row};

    let mut shelf = EnumColumn::<Shelf>::default();
    shelf.push(Shelf::Rare)?;
    shelf.push(Shelf::Fiction)?;

    assert_eq!(shelf.rows(), 2);
    assert_eq!(shelf.get(Row::of(0, 2, "book")?)?, Shelf::Rare);
    Ok(())
}
