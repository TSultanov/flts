import { writable } from "svelte/store";
import { eventHub } from "../data/tauri.svelte";
import type { UUID } from "../data/uuid";

export const bookDeletedNotice = writable(false);

const deletedBooks = new Set<UUID>();

export function isBookDeleted(bookId: UUID): boolean {
  return deletedBooks.has(bookId);
}

// Fires for every deletion, a local delete's echo included; only one that
// hits the open book leaves it.
export function watchBookDeleted(
  openBookId: () => UUID | null,
  leave: () => void,
): () => void {
  return eventHub.subscribe<UUID>(
    "book_deleted",
    () => true,
    (bookId) => {
      deletedBooks.add(bookId);
      if (bookId !== openBookId()) return;
      bookDeletedNotice.set(true);
      leave();
    },
  );
}
