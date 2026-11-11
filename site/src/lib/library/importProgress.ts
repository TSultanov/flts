import { writable } from "svelte/store";
import { eventHub } from "../data/tauri.svelte";

export type ImportProgress = { done: number; total: number };

// null unless a first-start library import is running.
export const importProgress = writable<ImportProgress | null>(null);

export function watchImportProgress(): () => void {
  const unsubscribers = [
    eventHub.subscribe<ImportProgress>(
      "library_import_progress",
      () => true,
      ({ done, total }) =>
        importProgress.set(done < total ? { done, total } : null),
    ),
    // Covers a missed final event.
    eventHub.subscribe(
      "library_updated",
      () => true,
      () => importProgress.set(null),
    ),
  ];
  return () => unsubscribers.forEach((u) => u());
}
