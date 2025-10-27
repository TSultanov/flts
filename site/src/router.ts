import { createRouter } from 'sv-router';
import LibraryView from './lib/LibraryView.svelte';
import ImportView from './lib/importView/ImportView.svelte';
import Config from './lib/Config.svelte';
import BookView from './lib/bookView/BookView.svelte';

export const { p, navigate, isActive, route } = createRouter({
	'/library': LibraryView,
	'/import': ImportView,
    '/config': Config,
    '/book/:bookId': BookView,
    '/book/:bookId/:chapterId': BookView,
});