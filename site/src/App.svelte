<script lang="ts">
    import {
        goto,
        Router,
        type RouteConfig,
    } from "@mateothegreat/svelte5-router";
    import type { RouterInstance } from "@mateothegreat/svelte5-router";
    import Config from "./lib/Config.svelte";
    import Nav from "./lib/Nav.svelte";
    import ImportView from "./lib/importView/ImportView.svelte";
    import { onMount, setContext } from "svelte";
    import LibraryView from "./lib/LibraryView.svelte";
    import { Library } from "./lib/data/library.svelte";
    import type { RouteLinkProps } from "./lib/Link.svelte";
    import BookView from "./lib/bookView/BookView.svelte";
    import { TranslationWorker } from "./lib/data/importWorker";
    import { createEvolu, getOrThrow, SimpleName } from "@evolu/common";
    import { evoluSvelteDeps } from "@evolu/svelte";
    import { Schema } from "./lib/data/evolu/schema";
    import { Books } from "./lib/data/evolu/book";
    import { Dictionary } from "./lib/data/evolu/dictionary";
    import { TranslationQueue } from "./lib/data/queueDb";

    const routes: RouteConfig[] = [
        {
            path: "",
            hooks: {
                pre: () => {
                    goto("/library");
                },
            },
        },
        {
            path: "/library",
            name: "Library",
            component: LibraryView,
        },
        {
            path: "/book/(?<bookId>[^/]+)(?:/(?<chapterId>[^/]+))?",
            name: "Book",
            component: BookView,
        },
        {
            path: "import",
            name: "Import",
            component: ImportView,
        },
        {
            path: "config",
            name: "Config",
            component: Config,
        },
    ];

    const links: RouteLinkProps[] = [
        {
            href: "/library",
            label: "Library",
        },
        {
            href: "/import",
            label: "Import",
        },
        {
            href: "/config",
            label: "Config",
        },
    ];

    let router = $state<RouterInstance | undefined>();

    let nav: HTMLElement | undefined = $state();
    const mainHeight: {
        value: number;
    } = $state({ value: 700 });

    setContext("mainHeight", mainHeight);

    function handleResize() {
        mainHeight.value = window.innerHeight - (nav?.clientHeight ?? 0);
    }

    const evolu = createEvolu(evoluSvelteDeps)(Schema, {
        name: getOrThrow(SimpleName.from("flts")),
        // syncUrl: "wss://your-sync-url", // optional, defaults to wss://free.evoluhq.com
        indexes: (create) => [
            // Foreign key indexes
            create("word_originalLanguageId_idx").on("word").column("originalLanguageId"),
            create("wordSpellingVariant_wordId_idx").on("wordSpellingVariant").column("wordId"),
            create("wordTranslation_translationLanguageId_idx").on("wordTranslation").column("translationLanguageId"),
            create("wordTranslation_originalWordVariantId_idx").on("wordTranslation").column("originalWordVariantId"),
            create("wordTranslationSpellingVariant_wordTranslationId_idx")
                .on("wordTranslationSpellingVariant")
                .column("wordTranslationId"),
            create("bookChapter_bookId_idx").on("bookChapter").column("bookId"),
            create("bookChapterParagraph_chapterId_idx").on("bookChapterParagraph").column("chapterId"),
            create("bookParagraphTranslation_chapterParagraphId_idx")
                .on("bookParagraphTranslation")
                .column("chapterParagraphId"),
            create("bookParagraphTranslation_languageId_idx")
                .on("bookParagraphTranslation")
                .column("languageId"),
            create("bookParagraphTranslationSentence_paragraphTranslationId_idx")
                .on("bookParagraphTranslationSentence")
                .column("paragraphTranslationId"),
            create("bookParagraphTranslationSentenceWord_sentenceId_idx")
                .on("bookParagraphTranslationSentenceWord")
                .column("sentenceId"),
            create("bookParagraphTranslationSentenceWord_wordTranslationId_idx")
                .on("bookParagraphTranslationSentenceWord")
                .column("wordTranslationId"),

            // Query-pattern indexes (single-column fallbacks)
            create("bookChapter_chapterIndex_idx").on("bookChapter").column("chapterIndex"),
            create("bookChapterParagraph_paragraphIndex_idx").on("bookChapterParagraph").column("paragraphIndex"),

            // Soft-delete filtering
            create("book_isDeleted_idx").on("book").column("isDeleted"),
            create("bookChapter_isDeleted_idx").on("bookChapter").column("isDeleted"),
            create("bookChapterParagraph_isDeleted_idx").on("bookChapterParagraph").column("isDeleted"),
            create("bookParagraphTranslation_isDeleted_idx").on("bookParagraphTranslation").column("isDeleted"),
            create("bookParagraphTranslationSentence_isDeleted_idx").on("bookParagraphTranslationSentence").column("isDeleted"),
            create("bookParagraphTranslationSentenceWord_isDeleted_idx")
                .on("bookParagraphTranslationSentenceWord")
                .column("isDeleted"),

            // Existing non-FK indexes
            create("bookParagraphTranslationSentence_sentenceIndex")
                .on("bookParagraphTranslationSentence")
                .column("sentenceIndex"),
            create("bookParagraphTranslationSentenceWord_wordIndex")
                .on("bookParagraphTranslationSentenceWord")
                .column("wordIndex"),
        ],
    });
    setContext("evolu", evolu); // TODO: improve DI
    const books = new Books(evolu);
    setContext("books", books);
    const dictionary = new Dictionary(evolu);
    const translationQueue = new TranslationQueue(evolu, books);
    setContext("translationQueue", translationQueue);
    const translationWorker = new TranslationWorker(
        evolu,
        books,
        dictionary,
        translationQueue,
    );
    translationWorker.startTranslations();

    if (typeof window !== 'undefined') {
        (window as any).evolu = evolu;
    }

    const library = new Library(evolu, books, translationQueue);
    setContext("library", library); // TODO: perhaps move library funcitonality into Books

    onMount(async () => {
        mainHeight.value = window.innerHeight - (nav?.clientHeight ?? 0);
        setContext("router", router);
    });
</script>

<svelte:window onresize={handleResize} />

<div bind:this={nav}>
    <Nav {router} {links} />
</div>
<div class="main" style="height: {mainHeight.value}px;">
    <Router bind:instance={router!} {routes} />
</div>

<style>
    .main {
        height: 100%;
    }
</style>
