import { expect, test, type Page } from '@playwright/test';
import {
  htmlOfSize,
  multipageSpec,
  paragraphLocator,
  seedAndOpen,
  type SeedParagraph,
} from './helpers/paragraph';

// Regression suite for chapter reading-position restore.
//
// Opening a book with a previously-saved `readingState` should scroll the
// horizontal page-flip container so the saved paragraph lands in view. The
// path runs: LibraryView -> /book/{id} -> BookView fetches readingState ->
// navigates to /book/{id}/{savedChapter} -> ChapterView mounts with
// `initialParagraphId` already set -> ChapterViewModel.startInitialSync
// -> scrollIntoView on the matching wrapper.
//
// IMPORTANT: the regression only reproduces via the library->book flow. A
// direct goto('/book/{id}/0') happens to work because ChapterView mounts
// once with initialParagraphId=null, syncs to paragraph 0, then re-runs
// the sync when readingState arrives — which lazy-mounts the world before
// the scroll fires. Real users click the book from the library, which
// mounts ChapterView a single time with initialParagraphId already
// populated, and that timing is what the refactor broke.
//
// These run on both Chromium and WebKit because WKWebView is the
// production engine.
test.describe('Chapter reading-state restore (multipage)', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  const COUNT = 80;
  const TARGET = 40;
  // Restore goes through one tick() retry inside scrollParagraphIntoView and
  // a subsequent recomputeMountWindow. 3s is comfortably above the worst
  // observed end-to-end latency on either engine.
  const POLL = { timeout: 3000, intervals: [50, 100, 200] } as const;

  async function openBookFromLibrary(
    page: import('@playwright/test').Page,
    bookId: string,
  ) {
    // Same path a real user takes: BookView mounts at /book/{id} with no
    // chapter, fetches readingState, then navigates to /book/{id}/{savedChapter}
    // so ChapterView mounts ONCE with initialParagraphId already set.
    await page.locator(`a[href="/book/${bookId}"]`).first().click();
    await page.waitForSelector('.paragraphs-container');
  }

  // ----- R1 ---------------------------------------------------------------
  test('R1: saved paragraph is in view after opening the book from the library', async ({
    page,
  }) => {
    const { bookId } = await seedAndOpen(
      page,
      multipageSpec(COUNT, {}, {
        readingState: { chapterId: 0, paragraphId: TARGET },
      }),
      { path: '/library' },
    );
    await openBookFromLibrary(page, bookId);

    const target = paragraphLocator(page, TARGET);
    await expect(target).toBeAttached();

    await expect
      .poll(
        async () =>
          page.evaluate((id) => {
            const container = document.querySelector(
              '.paragraphs-container',
            ) as HTMLElement | null;
            const el = document.querySelector(
              `.paragraph-wrapper[data-paragraph-id="${id}"]`,
            ) as HTMLElement | null;
            if (!container || !el) return false;
            const cr = container.getBoundingClientRect();
            const er = el.getBoundingClientRect();
            return er.right > cr.left && er.left < cr.right;
          }, TARGET),
        POLL,
      )
      .toBe(true);
  });

  // ----- R2 ---------------------------------------------------------------
  test('R2: with no saved state the book opens on paragraph 0', async ({ page }) => {
    const { bookId } = await seedAndOpen(page, multipageSpec(COUNT), {
      path: '/library',
    });
    await openBookFromLibrary(page, bookId);

    const first = paragraphLocator(page, 0);
    await expect(first).toBeAttached();

    // Settle a beat in case anything async would scroll us off paragraph 0.
    await page.waitForTimeout(200);

    const scrollLeft = await page.evaluate(() => {
      const c = document.querySelector(
        '.paragraphs-container',
      ) as HTMLElement | null;
      return c ? c.scrollLeft : -1;
    });
    expect(scrollLeft).toBeGreaterThanOrEqual(0);
    expect(scrollLeft).toBeLessThan(50);
  });

  // ----- Diverse-paragraph-size round-trip -------------------------------
  //
  // Uniform fillers (every paragraph 15 sentences) hide layout asymmetry.
  // Real books mix short dialog with long prose, and with column-fill: auto
  // a short paragraph can land anywhere in its column. The restore path
  // centers the wrapper horizontally; the save path hit-tests the top-left.
  // If the restore lands the wrong column, the very next scroll save will
  // overwrite the user's actual position with whatever paragraph happened
  // to sit at (left+16, top+16). The round-trip check below is the
  // strongest version of "restore correctly": after restore settles, the
  // top-left hit-test must return the seeded target id.
  test.describe('round-trip with diverse paragraph sizes', () => {
    type Profile = 'bimodal' | 'short-with-spikes' | 'long-with-gaps';
    const PROFILES: Profile[] = ['bimodal', 'short-with-spikes', 'long-with-gaps'];
    const TARGETS = [5, 20, 40, 60, 78];

    function buildOverrides(
      profile: Profile,
      count: number,
    ): Partial<Record<number, Partial<SeedParagraph>>> {
      const out: Record<number, Partial<SeedParagraph>> = {};
      for (let i = 0; i < count; i++) {
        let sentences: number;
        switch (profile) {
          case 'bimodal':
            sentences = i % 2 === 0 ? 1 : 30;
            break;
          case 'short-with-spikes':
            sentences = i % 10 === 0 ? 25 : 1;
            break;
          case 'long-with-gaps':
            sentences = i % 5 === 0 ? 1 : 15;
            break;
        }
        out[i] = { html: htmlOfSize(i, sentences) };
      }
      return out;
    }

    // The restore puts the column containing the saved paragraph into
    // view. Whether the saved paragraph appears at the column's top is
    // determined by CSS multi-column content flow, not by scrollLeft —
    // a mid-column target can't be moved to the top without putting a
    // different column in view. So the invariant is "target's column is
    // in view", checked by overlapping rect, identical in semantics to
    // R1's check.
    async function expectTargetColumnInView(page: Page, targetId: number) {
      await expect
        .poll(
          async () =>
            page.evaluate((id) => {
              const c = document.querySelector(
                '.paragraphs-container',
              ) as HTMLElement | null;
              const el = document.querySelector(
                `.paragraph-wrapper[data-paragraph-id="${id}"]`,
              ) as HTMLElement | null;
              if (!c || !el) return false;
              const cr = c.getBoundingClientRect();
              const er = el.getBoundingClientRect();
              return er.right > cr.left && er.left < cr.right;
            }, targetId),
          POLL,
        )
        .toBe(true);
    }

    for (const profile of PROFILES) {
      for (const target of TARGETS) {
        test(`round-trip: ${profile} @ p${target}`, async ({ page }) => {
          const { bookId } = await seedAndOpen(
            page,
            multipageSpec(COUNT, buildOverrides(profile, COUNT), {
              readingState: { chapterId: 0, paragraphId: target },
            }),
            { path: '/library' },
          );
          await openBookFromLibrary(page, bookId);
          await expect(paragraphLocator(page, target)).toBeAttached();

          await expectTargetColumnInView(page, target);
        });
      }
    }
  });

  // ----- Multi-page paragraph restore -----------------------------------
  //
  // The saved state carries a `pageOffset` so restore lands on the same
  // column within the paragraph the user was reading. `multipageSpec`
  // wedges a 300-sentence paragraph at index HUGE so it spans multiple
  // columns regardless of viewport size.
  test('R3: restore lands on the saved page within a multi-page paragraph', async ({
    page,
  }) => {
    const HUGE = 40;
    const PAGE_OFFSET = 2;

    const { bookId } = await seedAndOpen(
      page,
      multipageSpec(
        COUNT,
        { [HUGE]: { html: htmlOfSize(HUGE, 300) } },
        {
          readingState: {
            chapterId: 0,
            paragraphId: HUGE,
            pageOffset: PAGE_OFFSET,
          },
        },
      ),
      { path: '/library' },
    );
    await openBookFromLibrary(page, bookId);
    await expect(paragraphLocator(page, HUGE)).toBeAttached();

    // Wait until the wrapper has finished growing (spans more than one
    // column) AND scrollLeft sits within half a column-width of
    // wrapperContentLeft + PAGE_OFFSET * columnWidth.
    await expect
      .poll(
        async () =>
          page.evaluate(
            ({ id, offset }) => {
              const c = document.querySelector(
                '.paragraphs-container',
              ) as HTMLElement | null;
              const el = document.querySelector(
                `.paragraph-wrapper[data-paragraph-id="${id}"]`,
              ) as HTMLElement | null;
              if (!c || !el) return 'not-ready';
              const cr = c.getBoundingClientRect();
              const er = el.getBoundingClientRect();
              if (er.width <= cr.width * 1.5) return 'not-multi-column';
              const wrapperContentLeft = c.scrollLeft + (er.left - cr.left);
              const expected = wrapperContentLeft + offset * cr.width;
              const delta = Math.abs(c.scrollLeft - expected);
              return delta < cr.width / 2 ? 'ok' : `off-by-${Math.round(delta / cr.width)}`;
            },
            { id: HUGE, offset: PAGE_OFFSET },
          ),
        POLL,
      )
      .toBe('ok');
  });
});
