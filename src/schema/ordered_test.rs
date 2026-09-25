use proptest::prelude::*;

use crate::error::Result;
use crate::id::Id;
use crate::schema::{Holds, MergeInput, Store};
use crate::storage::{ColumnFile, WriterId};
use crate::test_utils::{TempDir, props};

#[crate::field_enum]
#[derive(PartialEq, Eq, Debug)]
enum Stage {
    New,
    Learning,
    Review,
}

#[crate::entity]
struct Book {
    title: String,
    chapters: Many<Chapter>,
    steps: Many<Step>,
}

#[crate::entity]
struct Chapter {
    #[order(book)]
    pos: u32,
    book: One<Book>,
    title: String,
    paragraphs: Many<Paragraph>,
}

#[crate::entity]
struct Paragraph {
    #[order(chapter)]
    pos: i64,
    chapter: One<Chapter>,
    text: String,
}

#[crate::entity]
struct Card {
    #[sorted]
    due: u64,
    #[sorted]
    interval: Option<u32>,
    #[sorted]
    stage: Stage,
    lemma: String,
}

#[crate::entity]
#[merge(with = summed)]
struct Step {
    #[order(book)]
    pos: u32,
    book: One<Book>,
    label: String,
}

fn summed(input: &MergeInput<'_, StepValue>) -> StepValue {
    let first = &input.candidates[0].value;
    StepValue {
        pos: input.candidates.iter().map(|held| held.value.pos).sum(),
        book: first.book,
        label: first.label.clone(),
    }
}

#[crate::container(schema = "ordered", version = 1)]
struct Shelf {
    books: Set<Book>,
    chapters: Set<Chapter>,
    paragraphs: Set<Paragraph>,
    cards: Set<Card>,
    steps: Set<Step>,
}

mod sorting {
    #[crate::entity]
    pub struct Book {
        title: String,
        chapters: Many<Chapter>,
    }

    #[crate::entity]
    pub struct Chapter {
        #[order(book)]
        pos: u32,
        book: One<Book>,
        title: String,
    }

    #[crate::entity]
    pub struct Card {
        #[sorted]
        due: u64,
    }

    #[crate::container(schema = "dropping", version = 1)]
    pub struct Shelf {
        books: Set<Book>,
        chapters: Set<Chapter>,
        cards: Set<Card>,
    }
}

mod plain {
    #[crate::entity]
    pub struct Book {
        title: String,
        chapters: Many<Chapter>,
    }

    #[crate::entity]
    pub struct Chapter {
        pos: u32,
        book: One<Book>,
        title: String,
    }

    #[crate::entity]
    pub struct Card {
        due: u64,
    }

    #[crate::container(schema = "dropping", version = 1)]
    pub struct Shelf {
        books: Set<Book>,
        chapters: Set<Chapter>,
        cards: Set<Card>,
    }
}

fn device(named: &str) -> WriterId {
    WriterId::from_device_id(named)
}

fn titles(shelf: &Shelf, book: Id<Book>) -> Result<Vec<String>> {
    shelf
        .at(book)?
        .chapters()?
        .map(|chapter| Ok(chapter.title()?.into_owned()))
        .collect()
}

fn texts(shelf: &Shelf, chapter: Id<Chapter>) -> Result<Vec<String>> {
    shelf
        .at(chapter)?
        .paragraphs()?
        .map(|paragraph| Ok(paragraph.text()?.into_owned()))
        .collect()
}

fn lemmas(shelf: &Shelf, ids: Vec<Id<Card>>) -> Result<Vec<String>> {
    ids.into_iter()
        .map(|card| Ok(shelf.cards.lemma(card)?.into_owned()))
        .collect()
}

#[test]
fn chapters_come_back_by_their_position_not_by_when_they_were_written() -> Result<()> {
    let mut shelf = Shelf::default();
    let alice = shelf.books.push("Alice")?;
    shelf.chapters.push(30, alice, "The Pool of Tears")?;
    shelf.chapters.push(10, alice, "Down the Rabbit-Hole")?;
    shelf
        .chapters
        .push(20, alice, "The Rabbit Sends in a Little Bill")?;

    assert_eq!(
        titles(&shelf, alice)?,
        [
            "Down the Rabbit-Hole",
            "The Rabbit Sends in a Little Bill",
            "The Pool of Tears"
        ]
    );
    Ok(())
}

#[test]
fn editing_a_paragraph_keeps_its_place_and_moving_its_position_moves_it() -> Result<()> {
    let mut shelf = Shelf::default();
    let alice = shelf.books.push("Alice")?;
    let rabbit = shelf.chapters.push(1, alice, "Down the Rabbit-Hole")?;
    let first = shelf.paragraphs.push(-1, rabbit, "Alice was tired.")?;
    shelf.paragraphs.push(0, rabbit, "So she considered.")?;
    shelf
        .paragraphs
        .push(1, rabbit, "There was nothing remarkable.")?;

    shelf.paragraphs.set_text(first, "Alice was very tired.")?;
    assert_eq!(
        texts(&shelf, rabbit)?,
        [
            "Alice was very tired.",
            "So she considered.",
            "There was nothing remarkable."
        ]
    );

    shelf.paragraphs.set_pos(first, 5)?;
    assert_eq!(
        texts(&shelf, rabbit)?,
        [
            "So she considered.",
            "There was nothing remarkable.",
            "Alice was very tired."
        ]
    );
    Ok(())
}

#[test]
fn equal_positions_fall_back_to_the_order_the_children_were_made_in() -> Result<()> {
    let mut shelf = Shelf::default();
    let alice = shelf.books.push("Alice")?;
    let rabbit = shelf.chapters.push(1, alice, "Down the Rabbit-Hole")?;
    for text in ["one", "two", "three"] {
        shelf.paragraphs.push(7, rabbit, text)?;
    }
    shelf.paragraphs.push(3, rabbit, "first")?;

    assert_eq!(texts(&shelf, rabbit)?, ["first", "one", "two", "three"]);
    Ok(())
}

#[test]
fn a_range_of_positions_names_only_the_children_of_the_parent_asked_about() -> Result<()> {
    let mut shelf = Shelf::default();
    let alice = shelf.books.push("Alice")?;
    let kokoro = shelf.books.push("Kokoro")?;
    let tears = shelf.chapters.push(3, alice, "The Pool of Tears")?;
    let hole = shelf.chapters.push(1, alice, "Down the Rabbit-Hole")?;
    shelf.chapters.push(2, kokoro, "Sensei and I")?;
    let bill = shelf
        .chapters
        .push(2, alice, "The Rabbit Sends in a Little Bill")?;

    assert_eq!(shelf.chapters.by_pos(alice, 1..3)?, [hole, bill]);
    assert_eq!(shelf.chapters.by_pos(alice, 2..)?, [bill, tears]);
    assert_eq!(shelf.chapters.by_pos(alice, ..=1)?, [hole]);
    assert!(shelf.chapters.by_pos(alice, 4..)?.is_empty());
    assert!(shelf.chapters.by_pos(alice, 2..2)?.is_empty());
    assert_eq!(shelf.chapters.by_pos(kokoro, ..)?.len(), 1);
    Ok(())
}

#[test]
fn a_removed_child_leaves_the_order_and_its_restoration_puts_it_back() -> Result<()> {
    let dir = TempDir::new("ordered_remove");
    let mut shelf = Shelf::create(dir.join("shelf.dbs"), device("here"))?;
    let alice = shelf.books.push("Alice")?;
    let hole = shelf.chapters.push(1, alice, "Down the Rabbit-Hole")?;
    shelf.chapters.push(2, alice, "The Pool of Tears")?;
    shelf.commit(device("here"))?;
    let saved = shelf.at(hole)?.version()?.dot();

    shelf.remove(hole)?;
    assert_eq!(titles(&shelf, alice)?, ["The Pool of Tears"]);
    assert_eq!(shelf.chapters.by_pos(alice, ..)?.len(), 1);

    shelf.restore(hole, saved)?;
    assert_eq!(
        titles(&shelf, alice)?,
        ["Down the Rabbit-Hole", "The Pool of Tears"]
    );
    Ok(())
}

#[test]
fn a_sorted_field_answers_a_range_across_the_whole_table() -> Result<()> {
    let mut shelf = Shelf::default();
    let casa = shelf.cards.push(30, Some(4), Stage::Review, "casa")?;
    let perro = shelf.cards.push(10, None, Stage::New, "perro")?;
    let gato = shelf.cards.push(20, Some(1), Stage::Learning, "gato")?;
    shelf.cards.push(40, Some(9), Stage::Review, "libro")?;

    assert_eq!(
        lemmas(&shelf, shelf.cards.by_due(..=30)?)?,
        ["perro", "gato", "casa"]
    );
    assert_eq!(shelf.cards.by_due(15..35)?, [gato, casa]);
    assert_eq!(
        lemmas(&shelf, shelf.cards.by_interval(..)?)?,
        ["gato", "casa", "libro"],
        "an absent interval is in no range of intervals"
    );
    assert_eq!(
        lemmas(&shelf, shelf.cards.by_stage(Stage::Learning..)?)?,
        ["gato", "casa", "libro"]
    );
    assert_eq!(shelf.cards.by_stage(..Stage::Learning)?, [perro]);

    shelf.cards.set_due(perro, 50)?;
    shelf.cards.remove(gato)?;
    assert_eq!(
        lemmas(&shelf, shelf.cards.by_due(..)?)?,
        ["casa", "libro", "perro"]
    );
    Ok(())
}

#[test]
fn a_run_folded_at_commit_serves_a_reopened_shelf_without_reading_the_text() -> Result<()> {
    let dir = TempDir::new("ordered_fold");
    let path = dir.join("shelf.dbs");
    let mut shelf = Shelf::create(&path, device("here"))?;
    let alice = shelf.books.push("Alice")?;
    let chapters: Vec<Id<Chapter>> = (0..4)
        .map(|at| shelf.chapters.push(3 - at, alice, &format!("chapter {at}")))
        .collect::<Result<_>>()?;
    for at in 0..600i64 {
        shelf
            .paragraphs
            .push(-at, chapters[at as usize % 4], &format!("paragraph {at}"))?;
    }
    shelf.commit(device("here"))?;

    let file = ColumnFile::open(&path)?;
    assert_eq!(
        file.column_meta("paragraph.pos.sorted")
            .map(|held| held.rows),
        Some(600),
        "a tail of 600 rows is worth folding at commit"
    );

    let reopened = Shelf::open(&path)?;
    let walked = texts(&reopened, chapters[1])?;
    assert_eq!(walked.len(), 150);
    assert_eq!(walked[0], "paragraph 597");
    assert_eq!(walked[149], "paragraph 1");
    let held = reopened
        .residency()
        .into_iter()
        .find(|held| held.name == "paragraph.text.bytes")
        .unwrap();
    assert!(held.resident > 0, "reading the texts read their bytes");
    let fresh = Shelf::open(&path)?;
    assert_eq!(fresh.paragraphs.by_pos(chapters[2], -10..=-2)?.len(), 3);
    for column in fresh.residency() {
        if column.name.starts_with("paragraph.text") {
            assert_eq!(column.resident, 0, "{} was read", column.name);
        }
    }
    reopened.verify()
}

#[test]
fn compaction_leaves_every_run_folded_to_the_last_row() -> Result<()> {
    let dir = TempDir::new("ordered_compact");
    let path = dir.join("shelf.dbs");
    let mut shelf = Shelf::create(&path, device("here"))?;
    let alice = shelf.books.push("Alice")?;
    shelf.chapters.push(2, alice, "two")?;
    shelf.chapters.push(1, alice, "one")?;
    shelf.cards.push(5, None, Stage::New, "casa")?;
    shelf.commit(device("here"))?;
    assert_eq!(Holds::<Chapter>::table(&shelf).pos.run().len(), 0);

    shelf.compact()?;

    assert_eq!(Holds::<Chapter>::table(&shelf).pos.run().len(), 2);
    assert_eq!(Holds::<Card>::table(&shelf).due.run().len(), 1);
    assert_eq!(titles(&shelf, alice)?, ["one", "two"]);
    shelf.verify()
}

#[test]
fn two_copies_that_ordered_one_book_apart_walk_it_the_same_way() -> Result<()> {
    let dir = TempDir::new("ordered_merge");
    let mut here = Shelf::create(dir.join("here.dbs"), device("here"))?;
    let alice = here.books.push("Alice")?;
    let hole = here.chapters.push(10, alice, "Down the Rabbit-Hole")?;
    let tears = here.chapters.push(20, alice, "The Pool of Tears")?;
    here.commit(device("here"))?;
    std::fs::copy(dir.join("here.dbs"), dir.join("there.dbs"))?;
    let mut there = Shelf::open(dir.join("there.dbs"))?;

    here.chapters.push(15, alice, "Written here")?;
    there.chapters.push(15, alice, "Written there")?;
    there.chapters.set_pos(hole, 30)?;
    here.chapters
        .set_title(tears, "The Pool of Tears, retitled")?;
    here.commit(device("here"))?;
    there.commit(device("there"))?;

    std::fs::copy(dir.join("there.dbs"), dir.join("other.dbs"))?;
    let mut other = Shelf::open(dir.join("other.dbs"))?;
    here.merge_from(dir.join("there.dbs"), device("here"))?;
    other.merge_from(dir.join("here.dbs"), device("there"))?;

    let walked = titles(&here, alice)?;
    assert_eq!(
        walked,
        [
            "Written here",
            "Written there",
            "The Pool of Tears, retitled",
            "Down the Rabbit-Hole"
        ]
    );
    assert_eq!(titles(&other, alice)?, walked);
    here.verify()?;
    other.verify()
}

#[test]
fn a_child_whose_position_a_merge_function_chose_is_ordered_by_that_choice() -> Result<()> {
    let dir = TempDir::new("ordered_resolved");
    let mut here = Shelf::create(dir.join("here.dbs"), device("here"))?;
    let alice = here.books.push("Alice")?;
    let summed = here.steps.push(1, alice, "summed")?;
    here.steps.push(5, alice, "five")?;
    here.steps.push(8, alice, "eight")?;
    here.commit(device("here"))?;
    std::fs::copy(dir.join("here.dbs"), dir.join("there.dbs"))?;
    let mut there = Shelf::open(dir.join("there.dbs"))?;

    here.steps.set_pos(summed, 3)?;
    there.steps.set_pos(summed, 4)?;
    here.commit(device("here"))?;
    there.commit(device("there"))?;
    here.merge_from(dir.join("there.dbs"), device("here"))?;

    assert_eq!(here.steps.pos(summed)?, 7);
    let walked: Vec<String> = here
        .at(alice)?
        .steps()?
        .map(|step| Ok(step.label()?.into_owned()))
        .collect::<Result<_>>()?;
    assert_eq!(walked, ["five", "summed", "eight"]);
    assert_eq!(here.steps.by_pos(alice, 6..=7)?, [summed]);
    assert!(here.steps.by_pos(alice, 3..=4)?.is_empty());
    Ok(())
}

#[test]
fn a_shelf_that_stopped_sorting_opens_the_file_and_drops_the_runs_it_left() -> Result<()> {
    let dir = TempDir::new("ordered_dropped");
    let path = dir.join("shelf.dbs");
    let mut shelf = sorting::Shelf::create(&path, device("here"))?;
    let alice = shelf.books.push("Alice")?;
    shelf.chapters.push(2, alice, "two")?;
    shelf.chapters.push(1, alice, "one")?;
    shelf.cards.push(5)?;
    shelf.commit(device("here"))?;
    shelf.compact()?;
    assert!(
        ColumnFile::open(&path)?
            .column_meta("chapter.pos.sorted")
            .is_some()
    );

    let mut plain = plain::Shelf::open(&path)?;
    plain.books.push("Kokoro")?;
    plain.commit(device("here"))?;

    let file = ColumnFile::open(&path)?;
    assert!(file.column_meta("chapter.pos.sorted").is_none());
    assert!(file.column_meta("card.due.sorted").is_none());
    let mut again = sorting::Shelf::open(&path)?;
    let walked: Vec<String> = again
        .at(alice)?
        .chapters()?
        .map(|chapter| Ok(chapter.title()?.into_owned()))
        .collect::<Result<_>>()?;
    assert_eq!(walked, ["one", "two"]);
    again.commit(device("here"))?;
    again.compact()?;
    assert!(
        ColumnFile::open(&path)?
            .column_meta("chapter.pos.sorted")
            .is_some()
    );
    again.verify()
}

#[derive(Clone, Debug)]
enum Deed {
    Push(u64, Option<u32>),
    Due(usize, u64),
    Remove(usize),
    Commit,
    Compact,
}

fn deeds() -> impl Strategy<Value = Vec<Deed>> {
    let due = prop_oneof![3 => 0u64..6, 1 => props::interesting_u64()];
    let interval = prop_oneof![1 => Just(None), 2 => (0u32..4).prop_map(Some)];
    prop::collection::vec(
        prop_oneof![
            6 => (due.clone(), interval).prop_map(|(due, interval)| Deed::Push(due, interval)),
            3 => (any::<usize>(), due).prop_map(|(at, due)| Deed::Due(at, due)),
            1 => any::<usize>().prop_map(Deed::Remove),
            1 => Just(Deed::Commit),
            1 => Just(Deed::Compact),
        ],
        0..40,
    )
}

proptest! {
    #![proptest_config(props::scaled(8))]

    #[test]
    fn every_range_of_due_dates_names_exactly_the_live_cards_a_scan_would(
        deeds in deeds(),
        low in prop_oneof![3 => 0u64..6, 1 => any::<u64>()],
        span in 0u64..8,
    ) {
        let dir = TempDir::new("ordered_prop");
        let mut shelf = Shelf::create(dir.join("shelf.dbs"), device("here")).unwrap();
        let mut model: Vec<(Id<Card>, u64, Option<u32>, bool)> = Vec::new();
        for deed in &deeds {
            match deed {
                Deed::Push(due, interval) => {
                    let card = shelf.cards.push(*due, *interval, Stage::New, "card").unwrap();
                    model.push((card, *due, *interval, true));
                }
                Deed::Due(at, due) => {
                    let live: Vec<usize> = (0..model.len()).filter(|at| model[*at].3).collect();
                    if let Some(&at) = live.get(at % live.len().max(1)) {
                        shelf.cards.set_due(model[at].0, *due).unwrap();
                        model[at].1 = *due;
                    }
                }
                Deed::Remove(at) => {
                    let live: Vec<usize> = (0..model.len()).filter(|at| model[*at].3).collect();
                    if let Some(&at) = live.get(at % live.len().max(1)) {
                        shelf.remove(model[at].0).unwrap();
                        model[at].3 = false;
                    }
                }
                Deed::Commit => {
                    shelf.commit(device("here")).unwrap();
                }
                Deed::Compact => {
                    shelf.commit(device("here")).unwrap();
                    shelf.compact().unwrap();
                }
            }
            let high = low.saturating_add(span);
            let mut expected: Vec<(u64, Id<Card>)> = model
                .iter()
                .filter(|(_, due, _, live)| *live && (low..=high).contains(due))
                .map(|(card, due, _, _)| (*due, *card))
                .collect();
            expected.sort();
            let expected: Vec<Id<Card>> = expected.into_iter().map(|(_, card)| card).collect();
            prop_assert_eq!(shelf.cards.by_due(low..=high).unwrap(), expected);

            let mut spaced: Vec<(u32, Id<Card>)> = model
                .iter()
                .filter_map(|(card, _, interval, live)| interval.filter(|_| *live).map(|interval| (interval, *card)))
                .collect();
            spaced.sort();
            let spaced: Vec<Id<Card>> = spaced.into_iter().map(|(_, card)| card).collect();
            prop_assert_eq!(shelf.cards.by_interval(..).unwrap(), spaced);
        }
        shelf.commit(device("here")).unwrap();
        shelf.verify().unwrap();
    }
}
