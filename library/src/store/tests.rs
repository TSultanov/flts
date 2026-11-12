use super::*;
use crate::test_utils::TempDir;
use isolang::Language;

fn book(title: &str) -> Book {
    let mut book = Book::create(Uuid::new_v4(), title, &Language::from_639_3("eng").unwrap());
    let first = book.push_chapter(Some("One"));
    book.push_paragraph(first, "alpha", Some("<p>alpha</p>"));
    book.push_paragraph(first, "beta", None);
    let second = book.push_chapter(None);
    book.push_paragraph(second, "gamma", None);
    book
}

fn version(paragraph: usize, timestamp: u64, text: &str) -> NewVersion {
    NewVersion {
        source_language: "eng".into(),
        target_language: "rus".into(),
        paragraph,
        translation: translation_import::ParagraphTranslation {
            timestamp,
            total_tokens: Some(7),
            sentences: vec![translation_import::Sentence {
                full_translation: text.into(),
                words: vec![],
            }],
        },
        model: "m".into(),
    }
}

fn copy_shards(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_shards(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn a_book_and_its_annotations_survive_a_reopen() {
    let dir = TempDir::new("flts_store");
    let root = dir.path.join("library");
    let source = book("Title");
    let id = source.id;
    {
        let store = LibraryStore::open(&root).unwrap();
        store
            .run_blocking(|db| {
                db.persist(
                    id,
                    &BookDelta {
                        book: Some(Arc::new(source.clone())),
                        versions: vec![version(1, 10, "old"), version(1, 20, "new")],
                        summaries: vec![(
                            1,
                            ChapterSummary {
                                generated: true,
                                model: "m".into(),
                                timestamp: 3,
                                text: "sum".into(),
                            },
                        )],
                        reading_state: Some(BookReadingState {
                            chapter_id: 1,
                            paragraph_id: 2,
                            page_offset: 3,
                        }),
                        folder_path: Some(vec!["a".into(), "b".into()]),
                    },
                )
            })
            .unwrap();
    }
    let store = LibraryStore::open(&root).unwrap();
    store
        .run_blocking(|db| {
            let listed = db.list_books()?;
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].title, "Title");
            assert_eq!(
                (listed[0].chapters_count, listed[0].paragraphs_count),
                (2, 3)
            );
            assert_eq!(listed[0].translations[0].translated_paragraphs_count, 1);
            assert_eq!(listed[0].folder_path, ["a", "b"]);

            let loaded = db.load_book(id)?;
            assert_eq!(loaded.info.chapter_count(), 2);
            let outline = db.outline(id)?;
            assert_eq!(outline.chapters[0].title.as_deref(), Some("One"));
            assert_eq!(outline.chapters[1].title, None);
            assert_eq!(db.paragraph(id, 0)?.html.as_deref(), Some("<p>alpha</p>"));
            assert_eq!(
                db.paragraph(id, outline.chapters[1].paragraphs[0])?.text,
                "gamma"
            );
            assert_eq!(listed[0].translations.len(), 1);
            let current = db.paragraph_translation(id, "rus", 1)?.unwrap().translation;
            assert_eq!(current.sentences[0].full_translation, "new");
            assert_eq!(current.total_tokens, Some(7));
            let versions = db.versions(id, "rus", 1)?;
            assert_eq!(versions.len(), 2);
            assert_eq!(versions[0].translation.timestamp, 10);
            assert!(loaded.annotations.summaries.entries[1].generated);
            assert!(!loaded.annotations.summaries.entries[0].generated);
            assert_eq!(
                loaded
                    .annotations
                    .user_state
                    .reading_state
                    .unwrap()
                    .page_offset,
                3
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn writing_the_same_delta_twice_adds_nothing() {
    let dir = TempDir::new("flts_store");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let source = Arc::new(book("Twice"));
    let id = source.id;
    store
        .run_blocking(|db| {
            for _ in 0..2 {
                db.persist(
                    id,
                    &BookDelta {
                        book: Some(source.clone()),
                        versions: vec![version(0, 1, "x")],
                        ..BookDelta::default()
                    },
                )?;
            }
            assert_eq!(db.version_count(id, "rus")?, 1);
            Ok(())
        })
        .unwrap();
}

#[test]
fn two_devices_converge_through_their_shards() {
    let dir = TempDir::new("flts_store");
    let (here, there) = (dir.path.join("a/library"), dir.path.join("b/library"));
    let a = LibraryStore::open(&here).unwrap();
    let b = LibraryStore::open(&there).unwrap();
    let source = Arc::new(book("Shared"));
    let id = source.id;
    // Both devices import the same legacy book on their own.
    for store in [&a, &b] {
        store
            .run_blocking(|db| {
                db.persist(
                    id,
                    &BookDelta {
                        book: Some(source.clone()),
                        ..BookDelta::default()
                    },
                )
            })
            .unwrap();
    }
    a.run_blocking(|db| {
        db.persist(
            id,
            &BookDelta {
                versions: vec![version(0, 5, "from a")],
                ..BookDelta::default()
            },
        )
    })
    .unwrap();
    b.run_blocking(|db| {
        db.persist(
            id,
            &BookDelta {
                versions: vec![version(2, 6, "from b")],
                folder_path: Some(vec!["shelf".into()]),
                ..BookDelta::default()
            },
        )
    })
    .unwrap();

    copy_shards(a.shared_dir(), b.shared_dir());
    copy_shards(b.shared_dir(), a.shared_dir());
    for store in [&a, &b] {
        store
            .run_blocking(|db| {
                assert!(db.refresh()?.any());
                assert!(!db.refresh()?.any(), "a second refresh finds nothing new");
                let listed = db.list_books()?;
                assert_eq!(listed.len(), 1, "one book, not one per device");
                assert_eq!(listed[0].folder_path, ["shelf"]);
                let loaded = db.load_book(id)?;
                assert_eq!(loaded.info.paragraphs_count(), 3);
                assert_eq!(listed[0].translations.len(), 1);
                assert_eq!(db.translated(id, "rus", &[0, 1, 2])?, [true, false, true]);
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn a_deleted_book_is_gone_and_its_id_is_spent() {
    let dir = TempDir::new("flts_store");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let source = Arc::new(book("Doomed"));
    let id = source.id;
    store
        .run_blocking(|db| {
            db.persist(
                id,
                &BookDelta {
                    book: Some(source.clone()),
                    ..BookDelta::default()
                },
            )?;
            db.delete_book(id)?;
            assert!(db.list_books()?.is_empty());
            assert!(!db.has_book(id)?);
            assert!(db.knows_book(id)?);
            assert!(db.load_book(id).is_err());
            Ok(())
        })
        .unwrap();
}

#[test]
fn cards_union_their_parts_and_keep_anki_data_by_request() {
    let dir = TempDir::new("flts_store");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let example = |paragraph| Example {
        source: "s".into(),
        translation: "t".into(),
        book_id: Uuid::nil(),
        chapter: 0,
        paragraph,
    };
    let card = |translation: &str, paragraph| Card {
        version: 2,
        id: crate::card::card_id("spa", "rus", "poder"),
        lemma: "poder".into(),
        translations: BTreeMap::from([("verb".to_string(), vec![translation.to_string()])]),
        examples: vec![example(paragraph)],
        anki_data: None,
    };
    store
        .run_blocking(|db| {
            assert!(db.card("spa", "rus", "poder")?.is_none());
            assert!(db.stage_card("spa", "rus", "poder", &card("мочь", 0), None)?);
            assert!(db.stage_card("spa", "rus", "poder", &card("уметь", 1), None)?);
            db.publish()?;
            assert!(!db.stage_card("spa", "rus", "poder", &card("уметь", 1), None)?);
            assert!(!db.has_unpublished(), "a card already held stages nothing");
            let stored = db.card("spa", "rus", "poder")?.unwrap();
            assert_eq!(stored.translations["verb"], ["мочь", "уметь"]);
            assert_eq!(stored.examples.len(), 2);
            assert_eq!(stored.anki_data, None);

            let data = Some(AnkiData {
                state: AnkiState::Active,
                interval_days: Some(4.0),
                ease_factor: None,
                fsrs_difficulty: None,
                fsrs_stability: None,
            });
            db.stage_card("spa", "rus", "poder", &stored, Some(&data))?;
            db.publish()?;
            assert_eq!(db.card("spa", "rus", "poder")?.unwrap().anki_data, data);
            assert_eq!(db.card_pairs()?, ["spa-rus"]);
            assert_eq!(db.card_slugs("spa", "rus")?, ["poder"]);
            let all = db.cards_in_pair("spa", "rus")?;
            assert_eq!(
                all,
                [(
                    "poder".to_string(),
                    db.card("spa", "rus", "poder")?.unwrap()
                )]
            );
            assert!(db.cards_in_pair("eng", "rus")?.is_empty());
            Ok(())
        })
        .unwrap();
}

fn bare_card(slug: &str, anki_data: Option<AnkiData>) -> Card {
    Card {
        version: 2,
        id: crate::card::card_id("spa", "rus", slug),
        lemma: slug.into(),
        translations: BTreeMap::from([("verb".to_string(), vec!["x".to_string()])]),
        examples: vec![],
        anki_data,
    }
}

fn anki_data(state: AnkiState, interval_days: f64) -> Option<AnkiData> {
    Some(AnkiData {
        state,
        interval_days: Some(interval_days),
        ease_factor: None,
        fsrs_difficulty: None,
        fsrs_stability: None,
    })
}

#[test]
fn a_card_staged_to_keep_its_anki_data_takes_the_incoming_when_it_has_none() {
    let dir = TempDir::new("flts_store");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let incoming = anki_data(AnkiState::Active, 4.0);
    let newer = anki_data(AnkiState::Suspended, 9.0);
    store
        .run_blocking(|db| {
            db.stage_card("spa", "rus", "poder", &bare_card("poder", None), None)?;
            let with_data = bare_card("poder", incoming.clone());
            assert!(db.stage_card("spa", "rus", "poder", &with_data, None)?);
            assert_eq!(
                db.card_anki_data("spa", "rus", "poder")?,
                Some(incoming.clone())
            );

            // Stored data wins over a later incoming value, and over none.
            let other = bare_card("poder", newer.clone());
            assert!(!db.stage_card("spa", "rus", "poder", &other, None)?);
            assert!(!db.stage_card("spa", "rus", "poder", &bare_card("poder", None), None)?);
            assert_eq!(
                db.card_anki_data("spa", "rus", "poder")?,
                Some(incoming.clone())
            );

            // An explicit value still replaces it, with nothing included.
            assert!(db.stage_card("spa", "rus", "poder", &other, Some(&None))?);
            assert_eq!(db.card_anki_data("spa", "rus", "poder")?, Some(None));
            assert_eq!(db.card_anki_data("spa", "rus", "ausente")?, None);
            Ok(())
        })
        .unwrap();
}

#[test]
fn card_anki_data_is_set_only_over_the_expected_value() {
    let dir = TempDir::new("flts_store");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let first = anki_data(AnkiState::Active, 4.0);
    let second = anki_data(AnkiState::Suspended, 9.0);
    let (one, two) = (first.clone().unwrap(), second.clone().unwrap());
    store
        .run_blocking(|db| {
            assert!(!db.set_card_anki_data("spa", "rus", "poder", &None, &one)?);
            assert!(!db.has_unpublished(), "a missing card is not created");

            db.stage_card("spa", "rus", "poder", &bare_card("poder", None), None)?;
            db.publish()?;
            assert!(db.set_card_anki_data("spa", "rus", "poder", &None, &one)?);
            assert!(db.has_unpublished());
            db.publish()?;
            assert!(!db.set_card_anki_data("spa", "rus", "poder", &None, &two)?);
            assert!(db.set_card_anki_data("spa", "rus", "poder", &first, &one)?);
            assert!(!db.has_unpublished(), "an equal value stages nothing");

            assert!(db.set_card_anki_data("spa", "rus", "poder", &first, &two)?);
            let stored = db.card("spa", "rus", "poder")?.unwrap();
            assert_eq!(stored.anki_data, second);
            assert_eq!(stored.lemma, "poder");
            assert_eq!(stored.translations["verb"], ["x"]);
            Ok(())
        })
        .unwrap();
}

#[test]
fn pruning_keeps_the_current_reading_position() {
    let dir = TempDir::new("flts_store");
    let root = dir.path.join("library");
    let store = LibraryStore::open(&root).unwrap();
    let source = Arc::new(book("Pruned"));
    let id = source.id;
    store
        .run_blocking(|db| {
            db.persist(
                id,
                &BookDelta {
                    book: Some(source.clone()),
                    ..BookDelta::default()
                },
            )?;
            for page in 0..5 {
                db.persist(
                    id,
                    &BookDelta {
                        reading_state: Some(BookReadingState {
                            chapter_id: 0,
                            paragraph_id: 1,
                            page_offset: page,
                        }),
                        ..BookDelta::default()
                    },
                )?;
            }
            assert_eq!(db.prune(&[])?, 4);
            assert_eq!(db.user_state(id)?.reading_state.unwrap().page_offset, 4);
            Ok(())
        })
        .unwrap();
    drop(store);
    let reopened = LibraryStore::open(&root).unwrap();
    reopened
        .run_blocking(|db| {
            assert_eq!(db.user_state(id)?.reading_state.unwrap().page_offset, 4);
            assert_eq!(db.load_book(id)?.info.paragraphs_count(), 3);
            Ok(())
        })
        .unwrap();
}

fn holds(dir: &Path, needle: &[u8]) -> bool {
    std::fs::read_dir(dir).unwrap().flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            return holds(&path, needle);
        }
        let bytes = std::fs::read(&path).unwrap();
        let mut found = bytes.windows(needle.len()).any(|w| w == needle);
        // Chunks are zstd frames; look inside every one that decodes.
        for at in 0..bytes.len().saturating_sub(4) {
            if found {
                break;
            }
            if bytes[at..at + 4] == [0x28, 0xb5, 0x2f, 0xfd]
                && let Ok(plain) = zstd::stream::decode_all(&bytes[at..])
            {
                found = plain.windows(needle.len()).any(|w| w == needle);
            }
        }
        found
    })
}

fn secret_book() -> Book {
    let mut book = Book::create(
        Uuid::new_v4(),
        "Unmistakable Zebra Title",
        &Language::from_639_3("eng").unwrap(),
    );
    let chapter = book.push_chapter(Some("Quokka heading"));
    book.push_paragraph(
        chapter,
        "wombat paragraph text that must leave the disk",
        None,
    );
    book
}

#[test]
fn deleting_a_book_removes_its_text_from_every_file() {
    let dir = TempDir::new("flts_store");
    let root = dir.path.join("library");
    let store = LibraryStore::open(&root).unwrap();
    let source = Arc::new(secret_book());
    let id = source.id;
    store
        .run_blocking(|db| {
            db.persist(
                id,
                &BookDelta {
                    book: Some(source.clone()),
                    versions: vec![version(0, 1, "platypus translation")],
                    ..BookDelta::default()
                },
            )
        })
        .unwrap();
    let everywhere = |needle: &str| {
        holds(store.shared_dir(), needle.as_bytes()) || holds(store.local_dir(), needle.as_bytes())
    };
    assert!(
        everywhere("wombat paragraph"),
        "the scan can see stored text"
    );
    assert!(everywhere("platypus translation"));

    store.run_blocking(|db| db.delete_book(id)).unwrap();
    for gone in [
        "wombat paragraph",
        "platypus translation",
        "Zebra Title",
        "Quokka heading",
    ] {
        assert!(!everywhere(gone), "{gone:?} survived the delete");
    }
    drop(store);
    let reopened = LibraryStore::open(&root).unwrap();
    reopened
        .run_blocking(|db| {
            assert!(db.list_books()?.is_empty());
            assert!(db.knows_book(id)?);
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_delete_elsewhere_removes_the_text_this_device_wrote() {
    let dir = TempDir::new("flts_store");
    let a = LibraryStore::open(&dir.path.join("a/library")).unwrap();
    let b = LibraryStore::open(&dir.path.join("b/library")).unwrap();
    let source = Arc::new(secret_book());
    let id = source.id;
    a.run_blocking(|db| {
        db.persist(
            id,
            &BookDelta {
                book: Some(source.clone()),
                ..BookDelta::default()
            },
        )
    })
    .unwrap();
    copy_shards(a.shared_dir(), b.shared_dir());
    b.run_blocking(|db| {
        assert!(db.refresh()?.any());
        db.delete_book(id)
    })
    .unwrap();
    // The text is in a's shard, which only a may rewrite.
    copy_shards(b.shared_dir(), a.shared_dir());
    a.run_blocking(|db| {
        assert!(db.refresh()?.any());
        assert!(!db.has_book(id)?);
        Ok(())
    })
    .unwrap();
    assert!(!holds(a.shared_dir(), b"wombat paragraph"));
    assert!(!holds(a.local_dir(), b"wombat paragraph"));

    // And once a's rewritten shard reaches b, b holds none of it either.
    copy_shards(a.shared_dir(), b.shared_dir());
    b.run_blocking(|db| db.refresh().map(|_| ())).unwrap();
    assert!(!holds(b.shared_dir(), b"wombat paragraph"));
    assert!(!holds(b.local_dir(), b"wombat paragraph"));
}

/// Delivers the files `from` holds of `writers`, shards and what they have seen.
fn deliver(from: &LibraryStore, to: &LibraryStore, writers: &[u64]) {
    fn walk(from: &Path, to: &Path, writers: &[u64]) {
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let (path, target) = (entry.path(), to.join(entry.file_name()));
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                walk(&path, &target, writers);
            } else if writers
                .iter()
                .any(|w| name.starts_with(&format!("{w:016x}.")))
            {
                std::fs::create_dir_all(to).unwrap();
                std::fs::copy(path, target).unwrap();
            }
        }
    }
    walk(from.shared_dir(), to.shared_dir(), writers);
}

#[test]
fn a_retired_devices_shards_are_scrubbed_by_the_lowest_writer_left() {
    let dir = TempDir::new("flts_store");
    let a = LibraryStore::open(&dir.path.join("a/library")).unwrap();
    let b = LibraryStore::open(&dir.path.join("b/library")).unwrap();
    let c = LibraryStore::open(&dir.path.join("c/library")).unwrap();
    let source = Arc::new(secret_book());
    let id = source.id;
    a.run_blocking(|db| {
        db.persist(
            id,
            &BookDelta {
                book: Some(source.clone()),
                ..BookDelta::default()
            },
        )
    })
    .unwrap();
    let writer = |store: &LibraryStore| store.run_blocking(|db| Ok(db.writer())).unwrap();
    let gone = writer(&a);
    let (wb, wc) = (writer(&b), writer(&c));
    for store in [&b, &c] {
        deliver(&a, store, &[gone]);
        store.run_blocking(|db| db.refresh().map(|_| ())).unwrap();
    }
    // Each says what it has seen, so each knows the other is there.
    for store in [&b, &c] {
        store
            .run_blocking(|db| {
                db.persist(
                    id,
                    &BookDelta {
                        folder_path: Some(vec!["shelf".into()]),
                        ..BookDelta::default()
                    },
                )
            })
            .unwrap();
    }
    deliver(&b, &c, &[wb]);
    deliver(&c, &b, &[wc]);
    b.run_blocking(|db| {
        db.refresh()?;
        db.delete_book(id)
    })
    .unwrap();
    deliver(&b, &c, &[wb]);
    c.run_blocking(|db| db.refresh().map(|_| ())).unwrap();
    assert!(
        holds(b.shared_dir(), b"wombat paragraph"),
        "a never returns"
    );

    let (low, high) = match wb < wc {
        true => (&b, &c),
        false => (&c, &b),
    };
    assert_eq!(
        high.run_blocking(|db| db.maintain(&[gone]))
            .unwrap()
            .scrubbed,
        0,
        "left to the other device"
    );
    assert!(
        low.run_blocking(|db| db.maintain(&[gone]))
            .unwrap()
            .scrubbed
            > 0
    );
    assert_eq!(
        low.run_blocking(|db| db.maintain(&[gone]))
            .unwrap()
            .scrubbed,
        0
    );
    assert!(!holds(low.shared_dir(), b"wombat paragraph"));
    assert!(!holds(low.local_dir(), b"wombat paragraph"));

    // The other device takes the scrubbed shards in its stride.
    deliver(low, high, &[gone, writer(low)]);
    high.run_blocking(|db| {
        db.refresh()?;
        assert!(db.knows_book(id)? && !db.has_book(id)?);
        assert!(db.list_books()?.is_empty());
        Ok(())
    })
    .unwrap();
    assert!(!holds(high.shared_dir(), b"wombat paragraph"));

    // So does the retired device, should it come back after all.
    deliver(low, &a, &[gone, wb, wc]);
    let again = Arc::new(book("Written after returning"));
    let again_id = again.id;
    a.run_blocking(|db| {
        db.refresh()?;
        assert!(!db.has_book(id)?);
        db.create_book(again.clone())
    })
    .unwrap();
    deliver(&a, low, &[gone]);
    low.run_blocking(|db| {
        db.refresh()?;
        assert!(db.has_book(again_id)?);
        assert_eq!(db.list_books()?.len(), 1);
        Ok(())
    })
    .unwrap();
}

#[test]
fn only_files_that_stay_unusable_are_reported() {
    let dir = TempDir::new("flts_store");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let source = Arc::new(book("Healthy"));
    store.run_blocking(|db| db.create_book(source)).unwrap();
    let table = store.shared_dir().join("book");
    let in_transit = table.join(".syncthing.00000000000000aa.v1.dbs.tmp");
    let conflict = table.join("00000000000000aa.v1.sync-conflict-20260101-000000-ABCDEFG.dbs");
    for path in [&in_transit, &conflict] {
        std::fs::write(path, b"not a shard").unwrap();
    }

    let health = |refreshes: u32| {
        store
            .run_blocking(|db| {
                for _ in 0..refreshes {
                    db.refresh()?;
                }
                Ok(db.health())
            })
            .unwrap()
    };
    let early = health(1);
    assert!(
        early.strangers.is_empty(),
        "it may be gone by the next look"
    );
    let late = health(ODD_GRACE);
    assert_eq!(late.strangers, [conflict.clone()]);
    assert!(late.unreadable.is_empty());

    std::fs::remove_file(conflict).unwrap();
    assert!(health(1).strangers.is_empty());
}

fn reading(paragraph: usize) -> BookReadingState {
    BookReadingState {
        chapter_id: 0,
        paragraph_id: paragraph,
        page_offset: 0,
    }
}

fn position(paragraph: usize) -> BookDelta {
    BookDelta {
        reading_state: Some(reading(paragraph)),
        folder_path: Some(vec![format!("shelf {paragraph}")]),
        ..BookDelta::default()
    }
}

#[test]
fn an_imported_position_never_outranks_one_the_app_wrote() {
    let dir = TempDir::new("flts_store");
    let a = LibraryStore::open(&dir.path.join("a/library")).unwrap();
    let b = LibraryStore::open(&dir.path.join("b/library")).unwrap();
    let source = Arc::new(book("Read"));
    let id = source.id;
    a.run_blocking(|db| {
        db.create_book(source.clone())?;
        db.persist(id, &position(2))
    })
    .unwrap();
    // The other device imports its old files later, so with a newer stamp.
    b.run_blocking(|db| {
        db.stage(
            id,
            &BookDelta {
                book: Some(source.clone()),
                ..position(1)
            },
            true,
        )?;
        db.publish()?;
        let state = db.user_state(id)?;
        assert_eq!(state.reading_state, Some(reading(1)), "all it has so far");
        assert_eq!(state.folder_path, ["shelf 1"]);
        Ok(())
    })
    .unwrap();

    copy_shards(a.shared_dir(), b.shared_dir());
    copy_shards(b.shared_dir(), a.shared_dir());
    for store in [&a, &b] {
        store
            .run_blocking(|db| {
                db.refresh()?;
                let state = db.user_state(id)?;
                assert_eq!(state.reading_state, Some(reading(2)));
                assert_eq!(state.folder_path, ["shelf 2"]);
                Ok(())
            })
            .unwrap();
    }
    // A later import of still older files changes nothing either.
    b.run_blocking(|db| {
        db.stage(id, &position(0), true)?;
        db.publish()?;
        assert_eq!(db.user_state(id)?.reading_state, Some(reading(2)));
        Ok(())
    })
    .unwrap();
}

#[test]
fn deleting_a_book_leaves_the_cards_that_quote_it_whole() {
    let dir = TempDir::new("flts_store");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let source = Arc::new(book("Quoted"));
    let id = source.id;
    let card = Card {
        version: 2,
        id: crate::card::card_id("spa", "rus", "poder"),
        lemma: "poder".into(),
        translations: BTreeMap::from([("verb".to_string(), vec!["мочь".to_string()])]),
        examples: vec![Example {
            source: "a sentence of the book".into(),
            translation: "перевод".into(),
            book_id: id,
            chapter: 0,
            paragraph: 0,
        }],
        anki_data: None,
    };
    store
        .run_blocking(|db| {
            db.create_book(source.clone())?;
            db.stage_card("spa", "rus", "poder", &card, None)?;
            db.publish()?;
            db.delete_book(id)?;
            // The user's cards are theirs: a deleted book takes none of them.
            let kept = db.card("spa", "rus", "poder")?.expect("the card stays");
            assert_eq!(kept.examples.len(), 1);
            assert_eq!(kept.examples[0].source, "a sentence of the book");
            Ok(())
        })
        .unwrap();
}

#[test]
fn creating_a_book_again_completes_the_one_that_was_cut_short() {
    let dir = TempDir::new("flts_store");
    let a = LibraryStore::open(&dir.path.join("a/library")).unwrap();
    let b = LibraryStore::open(&dir.path.join("b/library")).unwrap();
    let first = Arc::new(book("Interrupted"));
    let id = first.id;
    a.run_blocking(|db| db.create_book(first.clone())).unwrap();
    // Only the book's own row made it: the rest of the commit never did.
    let table = <schema::Book as Entity>::NAME;
    copy_shards(&a.shared_dir().join(table), &b.shared_dir().join(table));
    b.run_blocking(|db| {
        db.refresh()?;
        assert!(db.list_books()?.is_empty());
        assert!(!db.has_title("Interrupted")?, "the title is not taken");
        Ok(())
    })
    .unwrap();

    let again = Arc::new(book("Interrupted"));
    assert_ne!(again.id, id);
    let created = b.run_blocking(|db| db.create_book(again)).unwrap();
    assert_eq!(created, id, "the same book, not a second one");
    b.run_blocking(|db| {
        assert_eq!(db.list_books()?.len(), 1);
        assert_eq!(db.load_book(id)?.info.paragraphs_count(), 3);
        Ok(())
    })
    .unwrap();

    // What the first attempt wrote, arriving after all, adds nothing.
    copy_shards(a.shared_dir(), b.shared_dir());
    b.run_blocking(|db| {
        db.refresh()?;
        assert_eq!(db.list_books()?.len(), 1);
        assert_eq!(db.load_book(id)?.info.paragraphs_count(), 3);
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_store_told_its_identity_is_shared_reopens_as_a_new_writer() {
    let dir = TempDir::new("flts_store");
    let root = dir.path.join("library");
    let store = LibraryStore::open(&root).unwrap();
    let source = Arc::new(book("Kept"));
    let id = source.id;
    store.run_blocking(|db| db.create_book(source)).unwrap();
    let before = store.run_blocking(|db| Ok(db.writer())).unwrap();
    std::fs::write(store.local_dir().join("rotate-writer"), b"").unwrap();
    drop(store);

    let store = LibraryStore::open(&root).unwrap();
    let after = store.run_blocking(|db| Ok(db.writer())).unwrap();
    assert_ne!(after, before);
    assert!(!store.local_dir().join("rotate-writer").exists());
    store
        .run_blocking(|db| {
            assert_eq!(db.load_book(id)?.info.paragraphs_count(), 3);
            db.persist(id, &position(1))?;
            assert_eq!(db.user_state(id)?.reading_state, Some(reading(1)));
            Ok(())
        })
        .unwrap();
    // The earlier writer's shards are another device's now, and still read.
    let (_dir, peer) = crate::test_utils::peer_view(&store);
    assert_eq!(peer.run_blocking(|db| db.list_books()).unwrap().len(), 1);
}

#[test]
fn two_copies_of_one_installation_keep_what_each_wrote() {
    let dir = TempDir::new("flts_store");
    let (first_root, second_root) = (
        dir.path.join("first/library"),
        dir.path.join("second/library"),
    );
    let first = LibraryStore::open(&first_root).unwrap();
    first
        .run_blocking(|db| db.create_book(Arc::new(book("Shared"))))
        .unwrap();
    drop(first);
    // The whole installation copied, local state included, as a restore does.
    copy_shards(&dir.path.join("first"), &dir.path.join("second"));
    let first = LibraryStore::open(&first_root).unwrap();
    let second = LibraryStore::open(&second_root).unwrap();
    let writer = |store: &LibraryStore| store.run_blocking(|db| Ok(db.writer())).unwrap();
    assert_eq!(writer(&first), writer(&second));
    first
        .run_blocking(|db| db.create_book(Arc::new(book("From the first"))))
        .unwrap();
    second
        .run_blocking(|db| db.create_book(Arc::new(book("From the second"))))
        .unwrap();

    // Each one's shards replace the other's in turn.
    deliver(&first, &second, &[writer(&first)]);
    second.run_blocking(|db| db.refresh().map(|_| ())).unwrap();
    deliver(&second, &first, &[writer(&first)]);
    first.run_blocking(|db| db.refresh().map(|_| ())).unwrap();
    for store in [&first, &second] {
        let mut titles: Vec<String> = store
            .run_blocking(|db| db.list_books())
            .unwrap()
            .into_iter()
            .map(|book| book.title)
            .collect();
        titles.sort();
        assert_eq!(titles, ["From the first", "From the second", "Shared"]);
    }
    // Each found rows under its name that it never wrote.
    for (store, root) in [(first, &first_root), (second, &second_root)] {
        assert!(
            store
                .run_blocking(|db| Ok(db.health()))
                .unwrap()
                .shared_identity
        );
        let shared = writer(&store);
        drop(store);
        let store = LibraryStore::open(root).unwrap();
        assert_ne!(writer(&store), shared, "a writer of its own from here on");
        store
            .run_blocking(|db| {
                assert_eq!(db.list_books()?.len(), 3);
                assert!(db.health().shared_identity, "until the user has seen to it");
                db.settle_shared_identity()?;
                assert!(!db.health().shared_identity);
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn compaction_waits_for_a_quiet_moment_and_leaves_readers_alone() {
    let dir = TempDir::new("flts_store");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let source = Arc::new(book("Busy"));
    let id = source.id;
    store
        .run_blocking(|db| {
            db.create_book(source)?;
            db.persist(id, &position(1))
        })
        .unwrap();
    assert!(!store.compact_if_idle(Duration::from_secs(3600)).unwrap());
    assert!(store.compact_if_idle(Duration::ZERO).unwrap());
    store
        .run_blocking(|db| {
            assert_eq!(db.load_book(id)?.info.paragraphs_count(), 3);
            db.persist(id, &position(2))?;
            assert_eq!(db.user_state(id)?.reading_state, Some(reading(2)));
            Ok(())
        })
        .unwrap();
    drop(store);
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    store
        .run_blocking(|db| {
            assert_eq!(db.user_state(id)?.reading_state, Some(reading(2)));
            Ok(())
        })
        .unwrap();
}

#[test]
fn rows_that_stay_unapplied_are_reported_once_they_have_stayed() {
    let dir = TempDir::new("flts_store");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let waiting = dbsync::schema::MergeReport {
        waiting: 3,
        ..Default::default()
    };
    store
        .run_blocking(|db| {
            for _ in 1..ODD_GRACE {
                db.note_health(&waiting);
                assert!(!db.health().stalled, "it may be a matter of order");
            }
            db.note_health(&waiting);
            assert!(db.health().stalled);
            db.note_health(&Default::default());
            assert!(!db.health().stalled);
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_store_can_start_over_as_a_new_writer_without_reopening() {
    let dir = TempDir::new("flts_store");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let source = Arc::new(book("Kept"));
    let id = source.id;
    store
        .run_blocking(|db| {
            db.create_book(source)?;
            db.note_shared_identity()
        })
        .unwrap();
    let before = store.run_blocking(|db| Ok(db.writer())).unwrap();

    store.start_over_as_new_writer().unwrap();
    let after = store.run_blocking(|db| Ok(db.writer())).unwrap();
    assert_ne!(after, before);
    let own = store
        .shared_dir()
        .join("book")
        .join(format!("{after:016x}.v1.dbs"));
    assert!(store.is_own_file(&own));
    store
        .run_blocking(|db| {
            assert_eq!(db.load_book(id)?.info.paragraphs_count(), 3);
            db.persist(id, &position(1))?;
            assert!(db.health().shared_identity, "that is the user's to settle");
            db.settle_shared_identity()?;
            Ok(())
        })
        .unwrap();
    drop(store);
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    store
        .run_blocking(|db| {
            assert_eq!(db.writer(), after);
            assert_eq!(db.user_state(id)?.reading_state, Some(reading(1)));
            assert!(!db.health().shared_identity);
            Ok(())
        })
        .unwrap();
}
