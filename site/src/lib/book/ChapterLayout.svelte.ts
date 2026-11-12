// Any change inside the columns lays out all the text in them, so a long
// chapter pays for every page on each flip and tap. Here a paragraph far from
// the reader is drawn as a block taking exactly the room its text took: the
// pages stay where they are, and a few of them are left to lay out.

/**
 * The room a paragraph's text took: so many column ends, then so much of the
 * column it stops in (or, with no column end, of the one it starts in).
 * `lead` asks for a column end before it: the text began a column, and what
 * stands before it now does not end one.
 */
export type Extent = { lead: boolean; breaks: number; fill: number };

// A point in the flow of columns: which column, how far down it.
type Place = { column: number; y: number };
type Range = { from: number; to: number };
type Shape = {
  // Paragraphs before this index are drawn; the rest take no room yet.
  drawn: number;
  // Paragraphs before this index have a known extent.
  measured: number;
  // Measured paragraphs drawn as text; null while all are.
  near: Range | null;
};

// Pages on each side of the reader drawn as text, and the wider span a
// paragraph already drawn is kept for.
const NEAR_PAGES = 4;
const KEPT_PAGES = 5;

const HELD: Extent = Object.freeze({ lead: false, breaks: 0, fill: 0 });

const STORAGE_KEY = "flts.chapterLayout.v2";
const REMEMBERED_CHAPTERS = 40;

// An engine that twice failed to keep text in place between the blocks is
// not asked again this session.
let givenUp = false;

type Remembered = { extents: number[]; starts: number[]; columns: number };

// Newest last. Device-local: sizes belong to this screen and these fonts.
let remembered: Map<string, Remembered> | null = null;

function recall(): Map<string, Remembered> {
  if (remembered) return remembered;
  remembered = new Map();
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) remembered = new Map(JSON.parse(raw) as [string, Remembered][]);
  } catch {
    /* nothing remembered */
  }
  return remembered;
}

function persist(): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify([...recall()]));
  } catch {
    /* remembered for this session only */
  }
}

function between(start: Place, next: Place): Extent {
  const breaks = Math.max(0, next.column - start.column);
  return {
    lead: false,
    breaks,
    fill: Math.max(0, breaks > 0 ? next.y : next.y - start.y),
  };
}

function same(a: Extent, b: Extent): boolean {
  return a.breaks === b.breaks && Math.abs(a.fill - b.fill) < 0.5;
}

// Where boxes fall in the flow of columns.
function places(container: HTMLElement) {
  const frame = container.getBoundingClientRect();
  const scrollLeft = container.scrollLeft;
  const place = (rect: DOMRect | undefined, y: number | undefined) =>
    rect === undefined || y === undefined || frame.width <= 0
      ? null
      : {
          column: Math.round(
            (rect.left - frame.left + scrollLeft) / frame.width,
          ),
          y: y - frame.top,
        };
  return {
    start: (el: Element | undefined) => {
      const first = el?.getClientRects()[0];
      return place(first, first?.top);
    },
    end: (el: Element | undefined) => {
      const rects = el?.getClientRects();
      const last = rects?.[rects.length - 1];
      return place(last, last?.bottom);
    },
  };
}

/**
 * The columns the chapter's paragraphs take. Not from the scroll width:
 * that also counts the snap targets set out for a longer chapter.
 */
export function columnsOf(container: HTMLElement): number {
  const wrappers = container.getElementsByClassName("paragraph-wrapper");
  const end = places(container).end(wrappers[wrappers.length - 1]);
  return end === null ? 1 : end.column + 1;
}

export class ChapterLayout {
  #chapter: string;
  #count = 0;
  #indexOf = new Map<number, number>();
  #extents: Extent[] = [];
  #leading: Array<Extent | undefined> = [];
  // One more than extents: where the paragraph after the last would start.
  #starts: Place[] = [];
  #columns = 0;
  #shape: Shape = $state.raw({ drawn: Infinity, measured: 0, near: null });
  // Times in a row the blocks failed to keep the text in its place; an
  // engine that does not honour them gets the whole chapter drawn instead.
  #misplaced = $state(0);
  #misses = $state(0);

  constructor(chapter: string) {
    this.#chapter = chapter;
  }

  /** Every paragraph is drawn and has its place. */
  get whole(): boolean {
    return this.#count > 0 && this.#shape.measured >= this.#count;
  }

  /** Times the blocks were found wrong and everything drawn again. */
  get misses(): number {
    return this.#misses;
  }

  /** The blocks were given up: every paragraph is drawn as text. */
  get off(): boolean {
    return this.#misplaced >= 2 || givenUp;
  }

  #memo(page: HTMLElement): string {
    return `${this.#chapter}:${this.#count}:${page.clientWidth}x${page.clientHeight}`;
  }

  /**
   * A chapter of `ids`, the first `head` of them wanted at once. True when
   * its places are remembered for a page of this size, and nothing has to
   * be laid out to know them.
   */
  open(
    ids: readonly number[],
    head: number,
    page: HTMLElement | null,
  ): boolean {
    this.#count = ids.length;
    this.#indexOf = new Map(ids.map((id, i) => [id, i]));
    this.#extents = [];
    this.#leading = [];
    this.#starts = [];
    const known = page ? recall().get(this.#memo(page)) : undefined;
    if (
      known &&
      !this.off &&
      known.extents.length === ids.length * 2 &&
      known.starts.length === ids.length * 2 + 2
    ) {
      for (let i = 0; i < ids.length; i++) {
        this.#extents.push({
          lead: false,
          breaks: known.extents[2 * i],
          fill: known.extents[2 * i + 1],
        });
      }
      for (let i = 0; i <= ids.length; i++) {
        this.#starts.push({
          column: known.starts[2 * i],
          y: known.starts[2 * i + 1],
        });
      }
      this.#columns = known.columns;
      this.#shape = { drawn: ids.length, measured: ids.length, near: null };
      return true;
    }
    this.#shape = {
      drawn: this.off ? ids.length : Math.min(head, ids.length),
      measured: 0,
      near: null,
    };
    return false;
  }

  /** The block to draw instead of the paragraph's text, or null for text. */
  blockFor(paragraphId: number): Extent | null {
    // Read first: the shape is what tells the page to ask again.
    const { drawn } = this.#shape;
    const i = this.#indexOf.get(paragraphId);
    if (i === undefined || i >= drawn) return HELD;
    if (this.#isText(i)) return null;
    // Text that began a column was pushed there; a block is not, and
    // after text would start where the text before it ends.
    if (i > 0 && this.#starts[i].y < 0.5 && this.#isText(i - 1)) {
      return (this.#leading[i] ??= { ...this.#extents[i], lead: true });
    }
    return this.#extents[i];
  }

  #isText(i: number): boolean {
    const { drawn, measured, near } = this.#shape;
    if (i >= drawn) return false;
    return i >= measured || near === null || (i >= near.from && i <= near.to);
  }

  /** The column the paragraph starts in, if known. */
  columnOf(paragraphId: number): number | null {
    const i = this.#indexOf.get(paragraphId);
    if (i === undefined || i >= this.#shape.measured) return null;
    return this.#starts[i].column;
  }

  /** Draw at least the first `count` paragraphs now. */
  drawThrough(count: number): void {
    const drawn = Math.min(this.#count, count);
    if (drawn > this.#shape.drawn) this.#shape = { ...this.#shape, drawn };
  }

  /** The page changed size, or the text its shape: every place is void. */
  forget(page: HTMLElement | null): void {
    if (page && recall().delete(this.#memo(page))) persist();
    this.#extents = [];
    this.#leading = [];
    this.#starts = [];
    this.#shape = { drawn: this.#count, measured: 0, near: null };
  }

  /**
   * Measure what was drawn since the last call and draw a little more. Only
   * while the chapter is at rest on screen. True while there is more to do
   * that another frame can do.
   */
  advance(
    container: HTMLElement,
    hasText: (index: number) => boolean,
    slice: number,
  ): boolean {
    if (this.off || this.#count === 0) return false;
    let { drawn, measured } = this.#shape;
    drawn = Math.min(drawn, this.#count);
    if (measured >= this.#count) return false;

    for (let i = measured; i < drawn; i++) if (!hasText(i)) return false;
    const wrappers = container.getElementsByClassName("paragraph-wrapper");
    if (wrappers.length !== this.#count) return false;

    if (drawn > measured) {
      const at = places(container);
      const starts: Place[] = [];
      for (let i = measured; i < drawn; i++) {
        const start = at.start(wrappers[i]);
        if (start === null) return false;
        starts.push(start);
      }
      const whole = drawn === this.#count;
      if (whole) {
        const end = at.end(wrappers[drawn - 1]);
        if (end === null) return false;
        starts.push(end);
      }
      // The last one drawn stays text until what follows it is: only
      // then is it known where it ends.
      for (let k = 0; k < starts.length; k++)
        this.#starts[measured + k] = starts[k];
      for (let k = 0; k + 1 < starts.length; k++) {
        this.#extents[measured + k] = between(starts[k], starts[k + 1]);
      }
      measured = whole ? drawn : drawn - 1;
      if (whole) {
        this.#columns = columnsOf(container);
        this.#remember(container);
      }
    }

    let next = drawn;
    while (next < this.#count && next < drawn + slice && hasText(next)) next++;
    const more =
      measured < this.#count && (next > measured + 1 || next === this.#count);
    this.#shape = { ...this.#shape, drawn: next, measured };
    return more;
  }

  #remember(page: HTMLElement): void {
    const memo = this.#memo(page);
    const known = recall();
    known.delete(memo);
    known.set(memo, {
      extents: this.#extents.flatMap((e) => [e.breaks, e.fill]),
      starts: this.#starts.flatMap((s) => [s.column, s.y]),
      columns: this.#columns,
    });
    while (known.size > REMEMBERED_CHAPTERS) {
      known.delete(known.keys().next().value!);
    }
    persist();
  }

  /** The measured paragraphs with a part within `pages` of `page`. */
  around(page: number, pages: number): Range {
    const { measured } = this.#shape;
    const starts = this.#starts;
    let from = measured;
    let to = -1;
    for (let i = 0; i < measured; i++) {
      if (starts[i + 1].column < page - pages) continue;
      if (starts[i].column > page + pages) break;
      from = Math.min(from, i);
      to = i;
    }
    return { from, to };
  }

  /**
   * Draw as text the measured paragraphs on the pages around `page`. True
   * if that changed any.
   */
  moveNear(page: number): boolean {
    const { measured, near } = this.#shape;
    if (this.off || measured === 0) return false;
    let { from, to } = this.around(page, NEAR_PAGES);
    if (near !== null) {
      const kept = this.around(page, KEPT_PAGES);
      from = Math.min(from, Math.max(near.from, kept.from));
      to = Math.max(to, Math.min(near.to, kept.to));
    }
    if (near !== null && near.from === from && near.to === to) return false;
    this.#shape = { ...this.#shape, near: { from, to } };
    return true;
  }

  /**
   * With the blocks drawn: is the text between them the size it was measured
   * at, and the chapter as long? If not, everything is drawn again and
   * measured afresh; twice in a row, and the blocks are given up.
   */
  verify(container: HTMLElement): boolean {
    const { measured, near } = this.#shape;
    if (this.off || near === null || measured === 0) return true;
    const at = places(container);
    const wrappers = container.getElementsByClassName("paragraph-wrapper");
    let inPlace = wrappers.length === this.#count;
    const to = Math.min(near.to, measured - 1);
    for (let i = near.from; inPlace && i <= to; i++) {
      const start = at.start(wrappers[i]);
      const next =
        i + 1 < this.#count ? at.start(wrappers[i + 1]) : at.end(wrappers[i]);
      inPlace =
        start !== null &&
        next !== null &&
        start.column === this.#starts[i].column &&
        Math.abs(start.y - this.#starts[i].y) < 0.5 &&
        same(between(start, next), this.#extents[i]);
    }
    if (inPlace && this.whole && columnsOf(container) !== this.#columns) {
      inPlace = false;
    }
    if (inPlace) {
      this.#misplaced = 0;
      return true;
    }
    this.#misplaced++;
    this.#misses++;
    if (this.#misplaced >= 2) givenUp = true;
    this.forget(container);
    return false;
  }
}
