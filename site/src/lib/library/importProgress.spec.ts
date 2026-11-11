import { get } from "svelte/store";
import { describe, it, expect, vi, beforeEach } from "vitest";

const hub = vi.hoisted(() => {
  type Sub = {
    name: string;
    filter: (p: unknown) => boolean;
    handler: (p: unknown) => void;
    active: boolean;
  };
  const subs: Sub[] = [];
  return {
    subscribe(
      name: string,
      filter: (p: unknown) => boolean,
      handler: (p: unknown) => void,
    ): () => void {
      const sub: Sub = { name, filter, handler, active: true };
      subs.push(sub);
      return () => {
        sub.active = false;
      };
    },
    fire(name: string, payload: unknown): void {
      for (const s of [...subs]) {
        if (s.active && s.name === name && s.filter(payload)) {
          s.handler(payload);
        }
      }
    },
    reset(): void {
      subs.length = 0;
    },
  };
});

vi.mock("../data/tauri.svelte", () => ({ eventHub: hub }));

import { importProgress, watchImportProgress } from "./importProgress";

beforeEach(() => {
  hub.reset();
  importProgress.set(null);
});

describe("watchImportProgress", () => {
  it("is null until an import reports", () => {
    watchImportProgress();

    expect(get(importProgress)).toBeNull();
  });

  it("follows progress events", () => {
    watchImportProgress();

    hub.fire("library_import_progress", { done: 0, total: 12 });
    expect(get(importProgress)).toEqual({ done: 0, total: 12 });

    hub.fire("library_import_progress", { done: 3, total: 12 });
    expect(get(importProgress)).toEqual({ done: 3, total: 12 });
  });

  it("clears on the final event", () => {
    watchImportProgress();
    hub.fire("library_import_progress", { done: 11, total: 12 });

    hub.fire("library_import_progress", { done: 12, total: 12 });

    expect(get(importProgress)).toBeNull();
  });

  it("clears when the library updates", () => {
    watchImportProgress();
    hub.fire("library_import_progress", { done: 3, total: 12 });

    hub.fire("library_updated", []);

    expect(get(importProgress)).toBeNull();
  });

  it("stops reacting once unsubscribed", () => {
    const unwatch = watchImportProgress();
    unwatch();

    hub.fire("library_import_progress", { done: 3, total: 12 });

    expect(get(importProgress)).toBeNull();
  });
});
