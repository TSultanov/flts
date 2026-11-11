import { get } from "svelte/store";
import { describe, it, expect, vi, beforeEach } from "vitest";
import type { UUID } from "../data/uuid";

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

import {
  bookDeletedNotice,
  isBookDeleted,
  watchBookDeleted,
} from "./bookDeleted";

const OPEN = "aaaaaaaa-0000-0000-0000-000000000001" as UUID;
const OTHER = "bbbbbbbb-0000-0000-0000-000000000002" as UUID;
const THIRD = "cccccccc-0000-0000-0000-000000000003" as UUID;

beforeEach(() => {
  hub.reset();
  bookDeletedNotice.set(false);
});

describe("watchBookDeleted", () => {
  it("leaves the open book and raises the notice", () => {
    const leave = vi.fn();
    watchBookDeleted(() => OPEN, leave);

    hub.fire("book_deleted", OPEN);

    expect(leave).toHaveBeenCalledTimes(1);
    expect(get(bookDeletedNotice)).toBe(true);
    expect(isBookDeleted(OPEN)).toBe(true);
  });

  it("does nothing visible when another book is deleted", () => {
    const leave = vi.fn();
    watchBookDeleted(() => OPEN, leave);

    hub.fire("book_deleted", OTHER);

    expect(leave).not.toHaveBeenCalled();
    expect(get(bookDeletedNotice)).toBe(false);
    expect(isBookDeleted(OTHER)).toBe(true);
  });

  it("does nothing visible when no book is open", () => {
    const leave = vi.fn();
    watchBookDeleted(() => null, leave);

    hub.fire("book_deleted", THIRD);

    expect(leave).not.toHaveBeenCalled();
    expect(get(bookDeletedNotice)).toBe(false);
  });

  it("stops reacting once unsubscribed", () => {
    const leave = vi.fn();
    const unwatch = watchBookDeleted(() => THIRD, leave);
    unwatch();

    hub.fire("book_deleted", THIRD);

    expect(leave).not.toHaveBeenCalled();
    expect(get(bookDeletedNotice)).toBe(false);
  });
});
