use crate::error::Result;
use crate::id::Id;
use crate::schema::UnitEnum;
use crate::test_utils::props;
use proptest::prelude::*;

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
    author: One<Author>,
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
            "author.id",
            "author.id.buckets",
            "author.id.links",
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
fn an_id_no_generated_row_holds_is_refused_by_name() -> Result<()> {
    let mut authors = AuthorTable::default();
    authors.push("Only", None, 1, None, Shelf::Fiction)?;
    let stranger = Id::mint();

    let err = authors.name(stranger).unwrap_err();
    assert!(
        err.to_string().contains("author has no row with id"),
        "{err}"
    );
    assert!(err.to_string().contains(&stranger.to_string()), "{err}");
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

    let by_soseki = <Work as Child<Author>>::parents(&works).children_of(
        Table::ids(&works),
        Table::ids(&authors),
        soseki,
    )?;

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
    let anon = authors.push("Anonymous", None, 1200, None, Shelf::Reference)?;
    let file = committed(&path, |updater| authors.offer_to(updater));

    let mut read = AuthorTable::default();
    read.read_from(&file)?;

    assert_eq!(Table::rows(&read), 2);
    assert_eq!(read.pen_name(soseki)?.as_deref(), Some("S"));
    assert_eq!(read.shelf(soseki)?, Shelf::Rare);
    assert_eq!(read.died(anon)?, None);
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

struct Shelved {
    soseki: Id<Author>,
    anon: Id<Author>,
    kokoro: Id<Work>,
}

fn catalogued() -> Result<(Catalogue, Shelved)> {
    let mut catalogue = Catalogue::default();
    let soseki = catalogue.authors.push(
        "Natsume Kinnosuke",
        Some("Soseki"),
        1867,
        Some(1916),
        Shelf::Rare,
    )?;
    let anon = catalogue
        .authors
        .push("Anonymous", None, 1200, None, Shelf::Fiction)?;
    let kokoro = catalogue.works.push(soseki, "Kokoro")?;
    Ok((
        catalogue,
        Shelved {
            soseki,
            anon,
            kokoro,
        },
    ))
}

#[test]
fn a_generated_row_reads_every_kind_of_field_through_the_entity_that_owns_it() -> Result<()> {
    let (catalogue, on) = catalogued()?;

    let soseki = catalogue.at(on.soseki)?;
    assert_eq!(soseki.name()?, "Natsume Kinnosuke");
    assert_eq!(soseki.pen_name()?.as_deref(), Some("Soseki"));
    assert_eq!(soseki.born()?, 1867);
    assert_eq!(soseki.died()?, Some(1916));
    assert_eq!(soseki.shelf()?, Shelf::Rare);

    let anon = catalogue.at(on.anon)?;
    assert_eq!(anon.pen_name()?.as_deref(), None);
    assert_eq!(anon.died()?, None);
    Ok(())
}

#[test]
fn a_generated_row_follows_its_reference_into_the_row_it_points_at() -> Result<()> {
    let (catalogue, on) = catalogued()?;

    let kokoro = catalogue.at(on.kokoro)?;

    assert_eq!(kokoro.title()?, "Kokoro");
    assert_eq!(kokoro.author()?.name()?, "Natsume Kinnosuke");
    assert_eq!(kokoro.author()?.id()?, on.soseki);
    Ok(())
}

#[test]
fn a_generated_row_hands_back_the_children_a_many_field_declared() -> Result<()> {
    let (mut catalogue, on) = catalogued()?;
    catalogue.works.push(on.soseki, "Botchan")?;
    catalogue.works.push(on.anon, "Beowulf")?;

    let titles: Vec<String> = catalogue
        .at(on.soseki)?
        .works()?
        .map(|work| work.title().map(|title| title.into_owned()))
        .collect::<Result<_>>()?;

    assert_eq!(titles, ["Kokoro", "Botchan"]);
    assert_eq!(catalogue.at(on.anon)?.works()?.count(), 1);
    Ok(())
}

#[test]
fn a_generated_row_with_no_children_hands_back_nothing() -> Result<()> {
    let (catalogue, on) = catalogued()?;

    assert_eq!(catalogue.at(on.anon)?.works()?.count(), 0);
    Ok(())
}

#[test]
fn walking_a_generated_table_hands_back_one_row_handle_per_row() -> Result<()> {
    let (catalogue, _) = catalogued()?;

    let names: Vec<String> = catalogue
        .all::<Author>()
        .map(|author| author.name().map(|name| name.into_owned()))
        .collect::<Result<_>>()?;

    assert_eq!(names, ["Natsume Kinnosuke", "Anonymous"]);
    assert_eq!(catalogue.all::<Work>().count(), 1);
    Ok(())
}

#[test]
fn a_mutable_generated_row_writes_every_kind_of_field_through_the_entity() -> Result<()> {
    let (mut catalogue, on) = catalogued()?;

    let mut author = catalogue.at_mut(on.anon)?;
    author.set_name("Charles Dodgson")?;
    author.set_pen_name(Some("Lewis Carroll"))?;
    author.set_born(1832)?;
    author.set_died(Some(1898))?;
    author.set_shelf(Shelf::Reference)?;

    let author = catalogue.at(on.anon)?;
    assert_eq!(author.name()?, "Charles Dodgson");
    assert_eq!(author.pen_name()?.as_deref(), Some("Lewis Carroll"));
    assert_eq!(author.born()?, 1832);
    assert_eq!(author.died()?, Some(1898));
    assert_eq!(author.shelf()?, Shelf::Reference);
    Ok(())
}

#[test]
fn a_mutable_generated_row_clears_a_nullable_field_it_is_given_nothing_for() -> Result<()> {
    let (mut catalogue, on) = catalogued()?;

    let mut author = catalogue.at_mut(on.soseki)?;
    author.set_pen_name(None)?;
    author.set_died(None)?;

    let author = catalogue.at(on.soseki)?;
    assert_eq!(author.pen_name()?.as_deref(), None);
    assert_eq!(author.died()?, None);
    assert_eq!(author.name()?, "Natsume Kinnosuke");
    Ok(())
}

#[test]
fn a_mutable_generated_row_moves_a_work_to_another_author() -> Result<()> {
    let (mut catalogue, on) = catalogued()?;

    catalogue.at_mut(on.kokoro)?.set_author(on.anon)?;

    assert_eq!(catalogue.at(on.soseki)?.works()?.count(), 0);
    assert_eq!(
        catalogue.at(on.anon)?.works()?.next().unwrap().title()?,
        "Kokoro"
    );
    Ok(())
}

struct Penned {
    name: String,
    pen_name: Option<String>,
    born: u32,
    died: Option<u32>,
    shelf: Shelf,
}

#[derive(Debug, Clone)]
enum Deed {
    Write(String, Option<String>, u32, Option<u32>, Shelf),
    Publish(usize, String),
    Rename(usize, String),
    Repen(usize, Option<String>),
    Reborn(usize, u32),
    Redied(usize, Option<u32>),
    Reshelve(usize, Shelf),
    Reassign(usize, usize),
}

fn shelved() -> impl Strategy<Value = Shelf> {
    prop_oneof![
        Just(Shelf::Fiction),
        Just(Shelf::Reference),
        Just(Shelf::Rare),
    ]
}

fn written() -> impl Strategy<Value = (String, Option<String>, u32, Option<u32>, Shelf)> {
    (
        props::text(),
        props::optional_text(),
        props::interesting_u32(),
        proptest::option::of(props::interesting_u32()),
        shelved(),
    )
}

fn deed() -> impl Strategy<Value = Deed> {
    prop_oneof![
        5 => written().prop_map(|(name, pen, born, died, shelf)| Deed::Write(name, pen, born, died, shelf)),
        4 => (props::row_index(), props::text()).prop_map(|(at, title)| Deed::Publish(at, title)),
        2 => (props::row_index(), props::text()).prop_map(|(at, name)| Deed::Rename(at, name)),
        2 => (props::row_index(), props::optional_text()).prop_map(|(at, pen)| Deed::Repen(at, pen)),
        2 => (props::row_index(), props::interesting_u32()).prop_map(|(at, born)| Deed::Reborn(at, born)),
        2 => (props::row_index(), proptest::option::of(props::interesting_u32())).prop_map(|(at, died)| Deed::Redied(at, died)),
        2 => (props::row_index(), shelved()).prop_map(|(at, shelf)| Deed::Reshelve(at, shelf)),
        2 => (props::row_index(), props::row_index()).prop_map(|(at, to)| Deed::Reassign(at, to)),
    ]
}

fn deeds() -> impl Strategy<Value = Vec<Deed>> {
    prop::collection::vec(deed(), 0..24)
}

fn expected(deeds: &[Deed]) -> (Vec<Penned>, Vec<(usize, String)>) {
    let mut authors: Vec<Penned> = Vec::new();
    let mut works: Vec<(usize, String)> = Vec::new();

    for deed in deeds {
        match deed {
            Deed::Write(name, pen_name, born, died, shelf) => authors.push(Penned {
                name: name.clone(),
                pen_name: pen_name.clone(),
                born: *born,
                died: *died,
                shelf: *shelf,
            }),
            Deed::Publish(at, title) => {
                if !authors.is_empty() {
                    works.push((at % authors.len(), title.clone()));
                }
            }
            Deed::Rename(at, name) => {
                if !authors.is_empty() {
                    let row = at % authors.len();
                    authors[row].name = name.clone();
                }
            }
            Deed::Repen(at, pen_name) => {
                if !authors.is_empty() {
                    let row = at % authors.len();
                    authors[row].pen_name = pen_name.clone();
                }
            }
            Deed::Reborn(at, born) => {
                if !authors.is_empty() {
                    let row = at % authors.len();
                    authors[row].born = *born;
                }
            }
            Deed::Redied(at, died) => {
                if !authors.is_empty() {
                    let row = at % authors.len();
                    authors[row].died = *died;
                }
            }
            Deed::Reshelve(at, shelf) => {
                if !authors.is_empty() {
                    let row = at % authors.len();
                    authors[row].shelf = *shelf;
                }
            }
            Deed::Reassign(at, to) => {
                if !works.is_empty() {
                    let row = at % works.len();
                    works[row].0 = to % authors.len();
                }
            }
        }
    }
    (authors, works)
}

fn author_at(catalogue: &Catalogue, row: usize) -> Id<Author> {
    use crate::schema::{Row, Table};

    Table::id_at(&catalogue.authors, Row::trusted(row)).unwrap()
}

fn work_at(catalogue: &Catalogue, row: usize) -> Id<Work> {
    use crate::schema::{Row, Table};

    Table::id_at(&catalogue.works, Row::trusted(row)).unwrap()
}

fn author_row(catalogue: &Catalogue, of: Id<Author>) -> usize {
    use crate::schema::Table;

    Table::row_of(&catalogue.authors, of).unwrap().index()
}

fn work_row(catalogue: &Catalogue, of: Id<Work>) -> usize {
    use crate::schema::Table;

    Table::row_of(&catalogue.works, of).unwrap().index()
}

fn done(deeds: &[Deed]) -> Catalogue {
    use crate::schema::Table;

    let mut catalogue = Catalogue::default();

    for deed in deeds {
        let authors = Table::rows(&catalogue.authors);
        let works = Table::rows(&catalogue.works);
        match deed {
            Deed::Write(name, pen_name, born, died, shelf) => {
                catalogue
                    .authors
                    .push(name, pen_name.as_deref(), *born, *died, *shelf)
                    .unwrap();
            }
            Deed::Publish(at, title) => {
                if authors > 0 {
                    let wrote = author_at(&catalogue, at % authors);
                    catalogue.works.push(wrote, title).unwrap();
                }
            }
            Deed::Rename(at, name) => {
                if authors > 0 {
                    let held = author_at(&catalogue, at % authors);
                    catalogue.at_mut(held).unwrap().set_name(name).unwrap();
                }
            }
            Deed::Repen(at, pen_name) => {
                if authors > 0 {
                    let held = author_at(&catalogue, at % authors);
                    catalogue
                        .at_mut(held)
                        .unwrap()
                        .set_pen_name(pen_name.as_deref())
                        .unwrap();
                }
            }
            Deed::Reborn(at, born) => {
                if authors > 0 {
                    let held = author_at(&catalogue, at % authors);
                    catalogue.at_mut(held).unwrap().set_born(*born).unwrap();
                }
            }
            Deed::Redied(at, died) => {
                if authors > 0 {
                    let held = author_at(&catalogue, at % authors);
                    catalogue.at_mut(held).unwrap().set_died(*died).unwrap();
                }
            }
            Deed::Reshelve(at, shelf) => {
                if authors > 0 {
                    let held = author_at(&catalogue, at % authors);
                    catalogue.at_mut(held).unwrap().set_shelf(*shelf).unwrap();
                }
            }
            Deed::Reassign(at, to) => {
                if works > 0 {
                    let moved = work_at(&catalogue, at % works);
                    let home = author_at(&catalogue, to % authors);
                    catalogue.at_mut(moved).unwrap().set_author(home).unwrap();
                }
            }
        }
    }
    catalogue
}

fn penned_by_table(catalogue: &Catalogue) -> Vec<Penned> {
    use crate::schema::Table;

    (0..Table::rows(&catalogue.authors))
        .map(|row| {
            let of = author_at(catalogue, row);
            Penned {
                name: catalogue.authors.name(of).unwrap().into_owned(),
                pen_name: catalogue
                    .authors
                    .pen_name(of)
                    .unwrap()
                    .map(|pen| pen.into_owned()),
                born: catalogue.authors.born(of).unwrap(),
                died: catalogue.authors.died(of).unwrap(),
                shelf: catalogue.authors.shelf(of).unwrap(),
            }
        })
        .collect()
}

fn penned_by_row(catalogue: &Catalogue) -> Vec<Penned> {
    catalogue
        .all::<Author>()
        .map(|row| Penned {
            name: row.name().unwrap().into_owned(),
            pen_name: row.pen_name().unwrap().map(|pen| pen.into_owned()),
            born: row.born().unwrap(),
            died: row.died().unwrap(),
            shelf: row.shelf().unwrap(),
        })
        .collect()
}

proptest! {
    #[test]
    fn every_generated_field_a_row_hands_back_is_the_one_its_table_stores(deeds in deeds()) {
        let (authors, works) = expected(&deeds);
        let catalogue = done(&deeds);

        prop_assert_eq!(catalogue.all::<Author>().count(), authors.len());
        for (row, author) in authors.iter().enumerate() {
            let of = author_at(&catalogue, row);
            let held = catalogue.at(of).unwrap();
            prop_assert_eq!(held.name().unwrap().into_owned(), author.name.clone());
            prop_assert_eq!(
                held.pen_name().unwrap().map(|pen| pen.into_owned()),
                author.pen_name.clone()
            );
            prop_assert_eq!(held.born().unwrap(), author.born);
            prop_assert_eq!(held.died().unwrap(), author.died);
            prop_assert_eq!(held.shelf().unwrap(), author.shelf);
            prop_assert_eq!(held.name().unwrap().into_owned(), catalogue.authors.name(of).unwrap().into_owned());
        }

        prop_assert_eq!(catalogue.all::<Work>().count(), works.len());
        for (row, (author, title)) in works.iter().enumerate() {
            let held = catalogue.at(work_at(&catalogue, row)).unwrap();
            prop_assert_eq!(held.title().unwrap().into_owned(), title.clone());
            prop_assert_eq!(author_row(&catalogue, held.author().unwrap().id().unwrap()), *author);
        }
    }

    #[test]
    fn every_generated_child_is_found_under_exactly_one_parent(deeds in deeds()) {
        let (authors, works) = expected(&deeds);
        let catalogue = done(&deeds);

        let mut seen = Vec::new();
        for author in catalogue.all::<Author>() {
            let at = author_row(&catalogue, author.id().unwrap());
            let found: Vec<usize> = author
                .works()
                .unwrap()
                .map(|work| work_row(&catalogue, work.id().unwrap()))
                .collect();
            let scanned: Vec<usize> = works
                .iter()
                .enumerate()
                .filter(|(_, (owner, _))| *owner == at)
                .map(|(row, _)| row)
                .collect();
            prop_assert_eq!(&found, &scanned);
            seen.extend(found);
        }

        seen.sort_unstable();
        prop_assert_eq!(seen, (0..works.len()).collect::<Vec<_>>());
        prop_assert_eq!(catalogue.all::<Author>().count(), authors.len());
    }

    #[test]
    fn a_generated_field_written_through_a_row_is_the_only_field_that_changes(
        deeds in deeds(),
        at in props::row_index(),
        pen_name in props::optional_text(),
    ) {
        let mut catalogue = done(&deeds);
        catalogue
            .authors
            .push("Anchor", None, 1, None, Shelf::Fiction)
            .unwrap();
        let before = penned_by_table(&catalogue);
        let row = at % before.len();

        let held = author_at(&catalogue, row);
        catalogue
            .at_mut(held)
            .unwrap()
            .set_pen_name(pen_name.as_deref())
            .unwrap();

        let after = penned_by_row(&catalogue);
        prop_assert_eq!(after.len(), before.len());
        for (index, (was, now)) in before.iter().zip(&after).enumerate() {
            prop_assert_eq!(now.name.clone(), was.name.clone());
            prop_assert_eq!(now.born, was.born);
            prop_assert_eq!(now.died, was.died);
            prop_assert_eq!(now.shelf, was.shelf);
            let want = if index == row { pen_name.clone() } else { was.pen_name.clone() };
            prop_assert_eq!(now.pen_name.clone(), want);
        }
    }
}
