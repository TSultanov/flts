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
            path: "/book/(?<bookId>[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})(?:/(?<chapterId>[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}))?",
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
        name: getOrThrow(SimpleName.from("your-app-name")),
        // syncUrl: "wss://your-sync-url", // optional, defaults to wss://free.evoluhq.com
    });
    setContext("evolu", evolu); // TODO: improve DI
    const books = new Books(evolu);
    setContext("books", books);
    const dictionary = new Dictionary(evolu);
    const translationQueue = new TranslationQueue(evolu, books);
    setContext("translationQueue", translationQueue);
    const translationWorker = new TranslationWorker(evolu, books, dictionary, translationQueue);
    translationWorker.startTranslations();

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
