import { describe, it, expect, beforeEach, vi } from "vitest";

// Columns 100 wide and 100 high; each paragraph given as where it starts.
type At = [column: number, y: number];

function page(starts: At[], end: At) {
  const rect = ([column, y]: At, height: number) =>
    ({ left: column * 100, top: y, bottom: y + height }) as DOMRect;
  const wrappers = starts.map((at, i) => ({
    getClientRects: () => {
      const next = starts[i + 1] ?? end;
      // One box where it starts, one where it stops.
      return next[0] === at[0]
        ? [rect(at, next[1] - at[1])]
        : [rect(at, 100 - at[1]), rect([next[0], 0], next[1])];
    },
  }));
  return {
    clientWidth: 100,
    clientHeight: 100,
    scrollLeft: 0,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 100 }),
    getElementsByClassName: () => wrappers,
  } as unknown as HTMLElement;
}

// Twelve paragraphs of 40, so every third column starts with one.
const STARTS: At[] = Array.from({ length: 12 }, (_, i) => [
  Math.floor((i * 40) / 100),
  (i * 40) % 100,
]);
const END: At = [4, 80];
const ids = STARTS.map((_, i) => 100 + i);
const all = () => true;

// Vitest runs this module uncompiled; plain values do for what is tested.
vi.stubGlobal(
  "$state",
  Object.assign(<T>(v: T) => v, { raw: <T>(v: T) => v }),
);

async function fresh() {
  vi.resetModules();
  return (await import("./ChapterLayout.svelte")).ChapterLayout;
}

function measured(
  ChapterLayout: Awaited<ReturnType<typeof fresh>>,
  container: HTMLElement,
) {
  const layout = new ChapterLayout("book:1");
  layout.open(ids, 3, container);
  while (layout.advance(container, all, 4));
  return layout;
}

beforeEach(() => localStorage.clear());

describe("ChapterLayout", () => {
  it("draws the head as text and gives the rest no room", async () => {
    const layout = new (await fresh())("book:1");
    expect(layout.open(ids, 3, page(STARTS, END))).toBe(false);
    expect(layout.blockFor(ids[2])).toBeNull();
    expect(layout.blockFor(ids[3])).toEqual({
      lead: false,
      breaks: 0,
      fill: 0,
    });
    expect(layout.whole).toBe(false);
  });

  it("waits for text before drawing further", async () => {
    const layout = new (await fresh())("book:1");
    const container = page(STARTS, END);
    layout.open(ids, 3, container);
    expect(layout.advance(container, (i) => i < 3, 4)).toBe(false);
    expect(layout.blockFor(ids[3])).not.toBeNull();
  });

  it("measures a few paragraphs a call until the chapter is whole", async () => {
    const container = page(STARTS, END);
    const layout = new (await fresh())("book:1");
    layout.open(ids, 3, container);
    let calls = 1;
    while (layout.advance(container, all, 4)) calls++;
    expect(calls).toBeGreaterThan(2);
    expect(layout.whole).toBe(true);
    expect(layout.columnOf(ids[5])).toBe(2);
  });

  it("turns paragraphs far from the reader into blocks of their room", async () => {
    const container = page(
      // Sixty columns, so most of the chapter is far from either end.
      STARTS.map(([c, y]) => [c * 15, y] as At),
      [END[0] * 15, END[1]],
    );
    const layout = measured(await fresh(), container);
    layout.moveNear(0);
    expect(layout.blockFor(ids[0])).toBeNull();
    // Starts in column 15, the next one at the top of column 30.
    expect(layout.blockFor(ids[4])).toEqual({
      lead: false,
      breaks: 15,
      fill: 0,
    });
    layout.moveNear(30);
    expect(layout.blockFor(ids[5])).toBeNull();
    expect(layout.blockFor(ids[0])).not.toBeNull();
  });

  it("puts a column end before a block whose text began a column after text", async () => {
    const container = page(
      STARTS.map(([c, y]) => [c * 15, y] as At),
      [END[0] * 15, END[1]],
    );
    const layout = measured(await fresh(), container);
    // Paragraph 5 starts column 30 at its top; 4 is still text, 5 is not.
    layout.moveNear(15);
    expect(layout.blockFor(ids[4])).toBeNull();
    expect(layout.blockFor(ids[5])?.lead).toBe(true);
    expect(layout.blockFor(ids[7])?.lead).toBe(false);
  });

  it("remembers a measured chapter for a page of the same size", async () => {
    const ChapterLayout = await fresh();
    const container = page(STARTS, END);
    measured(ChapterLayout, container);

    const again = new ChapterLayout("book:1");
    expect(again.open(ids, 3, container)).toBe(true);
    expect(again.whole).toBe(true);

    const other = Object.assign(page(STARTS, END), { clientWidth: 90 });
    expect(new ChapterLayout("book:1").open(ids, 3, other)).toBe(false);
    expect(new ChapterLayout("book:2").open(ids, 3, container)).toBe(false);
  });

  it("drops what it remembered when the text no longer fits it", async () => {
    const ChapterLayout = await fresh();
    const container = page(STARTS, END);
    const layout = measured(ChapterLayout, container);
    layout.moveNear(0);
    expect(layout.verify(container)).toBe(true);

    // The same paragraphs, each now taking half a column.
    const moved = page(
      STARTS.map((_, i) => [Math.floor(i / 2), (i % 2) * 50] as At),
      [6, 0],
    );
    expect(layout.verify(moved)).toBe(false);
    expect(layout.misses).toBe(1);
    expect(layout.whole).toBe(false);
    expect(layout.blockFor(ids[11])).toBeNull();
    expect(new ChapterLayout("book:1").open(ids, 3, moved)).toBe(false);
  });

  it("gives the blocks up after two failures in a row", async () => {
    const ChapterLayout = await fresh();
    const container = page(STARTS, END);
    const moved = page(
      STARTS.map((_, i) => [Math.floor(i / 2), (i % 2) * 50] as At),
      [6, 0],
    );
    const layout = measured(ChapterLayout, container);
    for (let round = 0; round < 2; round++) {
      layout.moveNear(0);
      expect(layout.verify(moved)).toBe(false);
      while (layout.advance(container, all, 4));
    }
    expect(layout.off).toBe(true);
    // And for every chapter opened after it.
    const next = new ChapterLayout("book:3");
    next.open(ids, 3, container);
    expect(next.blockFor(ids[11])).toBeNull();
  });
});
