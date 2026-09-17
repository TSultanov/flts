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

#[crate::entity]
struct Author {
    name: String,
    pen_name: Option<String>,
    born: u32,
    died: Option<u32>,
    shelf: Shelf,
    works: Many<Work>,
}

#[crate::entity]
struct Work {
    author: Ref<Author>,
    title: String,
}

#[test]
fn a_generated_entity_names_its_columns_after_itself() {
    use crate::schema::Table;

    let names: Vec<String> = AuthorTable::default()
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect();

    assert_eq!(
        names,
        [
            "author.name",
            "author.name.bytes",
            "author.pen_name",
            "author.pen_name.bytes",
            "author.born",
            "author.died",
            "author.died.present",
            "author.shelf",
        ]
    );
}

#[test]
fn a_generated_table_reads_back_every_kind_of_field_it_was_pushed() -> Result<()> {
    let mut authors = AuthorTable::default();
    let soseki = authors.push(
        "Natsume Kinnosuke",
        Some("Soseki"),
        1867,
        Some(1916),
        Shelf::Rare,
    )?;
    let anon = authors.push("Anonymous", None, 1200, None, Shelf::Fiction)?;

    assert_eq!(authors.name(soseki)?, "Natsume Kinnosuke");
    assert_eq!(authors.pen_name(soseki)?.as_deref(), Some("Soseki"));
    assert_eq!(authors.born(soseki)?, 1867);
    assert_eq!(authors.died(soseki)?, Some(1916));
    assert_eq!(authors.shelf(soseki)?, Shelf::Rare);

    assert_eq!(authors.pen_name(anon)?.as_deref(), None);
    assert_eq!(authors.died(anon)?, None);
    Ok(())
}

#[test]
fn a_generated_setter_changes_one_field_and_leaves_the_rest() -> Result<()> {
    let mut authors = AuthorTable::default();
    let carroll = authors.push("Charles Dodgson", None, 1832, Some(1898), Shelf::Fiction)?;

    authors.set_pen_name(carroll, Some("Lewis Carroll"))?;
    authors.set_born(carroll, 1833)?;

    assert_eq!(authors.pen_name(carroll)?.as_deref(), Some("Lewis Carroll"));
    assert_eq!(authors.born(carroll)?, 1833);
    assert_eq!(authors.name(carroll)?, "Charles Dodgson");
    assert_eq!(authors.died(carroll)?, Some(1898));
    Ok(())
}

#[test]
fn an_id_past_the_last_generated_row_is_refused_by_name() -> Result<()> {
    use crate::id::Id;

    let mut authors = AuthorTable::default();
    authors.push("Only", None, 1, None, Shelf::Fiction)?;

    let err = authors.name(Id::from_index(1)).unwrap_err();
    assert!(err.to_string().contains("author has no row 1"), "{err}");
    Ok(())
}

#[test]
fn a_generated_entity_knows_whose_child_it_is() -> Result<()> {
    use crate::schema::{Child, Table};

    let mut authors = AuthorTable::default();
    let soseki = authors.push("Soseki", None, 1867, None, Shelf::Rare)?;
    let carroll = authors.push("Carroll", None, 1832, None, Shelf::Fiction)?;

    let mut works = WorkTable::default();
    works.push(soseki, "Kokoro")?;
    works.push(carroll, "Alice")?;
    works.push(soseki, "Botchan")?;

    let by_soseki =
        <Work as Child<Author>>::parents(&works).children_of(Table::rows(&authors), soseki)?;

    assert_eq!(by_soseki.len(), 2);
    assert_eq!(works.title(by_soseki[1])?, "Botchan");
    Ok(())
}

#[test]
fn a_generated_table_round_trips_through_a_file() -> Result<()> {
    use crate::schema::Table;
    use crate::test_utils::{TempDir, committed};

    let dir = TempDir::new("generated");
    let path = dir.join("authors.dbs");

    let mut authors = AuthorTable::default();
    let soseki = authors.push("Soseki", Some("S"), 1867, Some(1916), Shelf::Rare)?;
    authors.push("Anonymous", None, 1200, None, Shelf::Reference)?;
    let file = committed(&path, |updater| authors.offer_to(updater));

    let mut read = AuthorTable::default();
    read.read_from(&file)?;

    assert_eq!(Table::rows(&read), 2);
    assert_eq!(read.pen_name(soseki)?.as_deref(), Some("S"));
    assert_eq!(read.shelf(soseki)?, Shelf::Rare);
    assert_eq!(read.died(crate::id::Id::from_index(1))?, None);
    Ok(())
}

#[test]
fn every_column_a_table_names_is_a_column_it_reports_residency_for() {
    use crate::schema::Table;

    for (specs, held) in [
        (
            AuthorTable::default().specs(),
            AuthorTable::default().residency(),
        ),
        (
            WorkTable::default().specs(),
            WorkTable::default().residency(),
        ),
    ] {
        assert_eq!(
            held.into_iter().map(|held| held.name).collect::<Vec<_>>(),
            specs.into_iter().map(|spec| spec.name).collect::<Vec<_>>(),
            "a column that cannot say whether it is resident is a column nobody can tune"
        );
    }
}

#[crate::container]
struct Catalogue {
    authors: Set<Author>,
    works: Set<Work>,
}

fn catalogued() -> Result<Catalogue> {
    let mut catalogue = Catalogue::default();
    let soseki = catalogue.authors.push(
        "Natsume Kinnosuke",
        Some("Soseki"),
        1867,
        Some(1916),
        Shelf::Rare,
    )?;
    catalogue
        .authors
        .push("Anonymous", None, 1200, None, Shelf::Fiction)?;
    catalogue.works.push(soseki, "Kokoro")?;
    Ok(catalogue)
}

#[test]
fn a_generated_row_reads_every_kind_of_field_through_the_entity_that_owns_it() -> Result<()> {
    use crate::id::Id;

    let catalogue = catalogued()?;

    let soseki = catalogue.at(Id::<Author>::from_index(0))?;
    assert_eq!(soseki.name()?, "Natsume Kinnosuke");
    assert_eq!(soseki.pen_name()?.as_deref(), Some("Soseki"));
    assert_eq!(soseki.born()?, 1867);
    assert_eq!(soseki.died()?, Some(1916));
    assert_eq!(soseki.shelf()?, Shelf::Rare);

    let anon = catalogue.at(Id::<Author>::from_index(1))?;
    assert_eq!(anon.pen_name()?.as_deref(), None);
    assert_eq!(anon.died()?, None);
    Ok(())
}

#[test]
fn a_generated_row_follows_its_reference_into_the_row_it_points_at() -> Result<()> {
    use crate::id::Id;

    let catalogue = catalogued()?;

    let kokoro = catalogue.at(Id::<Work>::from_index(0))?;

    assert_eq!(kokoro.title()?, "Kokoro");
    assert_eq!(kokoro.author()?.name()?, "Natsume Kinnosuke");
    assert_eq!(kokoro.author()?.id(), Id::<Author>::from_index(0));
    Ok(())
}

#[test]
fn a_generated_row_hands_back_the_children_a_many_field_declared() -> Result<()> {
    use crate::id::Id;

    let mut catalogue = catalogued()?;
    let soseki = Id::<Author>::from_index(0);
    catalogue.works.push(soseki, "Botchan")?;
    catalogue.works.push(Id::from_index(1), "Beowulf")?;

    let titles: Vec<String> = catalogue
        .at(soseki)?
        .works()?
        .map(|work| work.title().map(|title| title.into_owned()))
        .collect::<Result<_>>()?;

    assert_eq!(titles, ["Kokoro", "Botchan"]);
    assert_eq!(
        catalogue.at(Id::<Author>::from_index(1))?.works()?.count(),
        1
    );
    Ok(())
}

#[test]
fn a_generated_row_with_no_children_hands_back_nothing() -> Result<()> {
    use crate::id::Id;

    let catalogue = catalogued()?;

    assert_eq!(
        catalogue.at(Id::<Author>::from_index(1))?.works()?.count(),
        0
    );
    Ok(())
}

#[test]
fn walking_a_generated_table_hands_back_one_row_handle_per_row() -> Result<()> {
    let catalogue = catalogued()?;

    let names: Vec<String> = catalogue
        .all::<Author>()
        .map(|author| author.name().map(|name| name.into_owned()))
        .collect::<Result<_>>()?;

    assert_eq!(names, ["Natsume Kinnosuke", "Anonymous"]);
    assert_eq!(catalogue.all::<Work>().count(), 1);
    Ok(())
}
