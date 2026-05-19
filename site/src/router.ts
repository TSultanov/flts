import { createRouter } from 'sv-router';
import LibraryView from './lib/library/LibraryView.svelte';
import ImportView from './lib/import/ImportView.svelte';
import ConfigView from './lib/config/ConfigView.svelte';
import BookView from './lib/book/BookView.svelte';
import LyricsView from './lib/lyrics/LyricsView.svelte';

export const { p, navigate, isActive, route } = createRouter({
	'/library': LibraryView,
	'/import': ImportView,
    '/config': ConfigView,
    '/book/:bookId': BookView,
    '/book/:bookId/:chapterId': BookView,
    '/lyrics': LyricsView,
});