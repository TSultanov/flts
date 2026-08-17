import { type Page } from '@playwright/test';
import { expect, test } from './helpers/test';
import {
  htmlOfSize,
  multipageSpec,
  paragraphLocator,
  seedAndOpen,
  type SeedParagraph,
} from './helpers/paragraph';

// Chapter reading-position restore, over both Chromium and WebKit (WKWebView
// is production).
//
// Specs must enter via the library->book flow: a direct goto('/book/{id}/0')
// mounts ChapterView with initialParagraphId=null and re-syncs later, which
// lazy-mounts everything before the scroll and hides the behaviour under test.
test.describe('Chapter reading-state restore (multipage)', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  const COUNT = 80;
  const TARGET = 40;
  // Covers scrollParagraphIntoView's tick() retry plus a mount-window recompute.
  const POLL = { timeout: 3000, intervals: [50, 100, 200] } as const;

  async function openBookFromLibrary(
    page: import('@playwright/test').Page,
    bookId: string,
  ) {
    // BookView must resolve the chapter, so ChapterView mounts once with
    // initialParagraphId already set.
    await page.locator(`a[href="/book/${bookId}"]`).first().click();
    await page.waitForSelector('.paragraphs-container');
  }

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

  test('R2: with no saved state the book opens on paragraph 0', async ({ page }) => {
    const { bookId } = await seedAndOpen(page, multipageSpec(COUNT), {
      path: '/library',
    });
    await openBookFromLibrary(page, bookId);

    const first = paragraphLocator(page, 0);
    await expect(first).toBeAttached();

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

  // Mixed paragraph sizes: restore centers the wrapper while save hit-tests
  // the top-left, so a restore into the wrong column silently overwrites the
  // user's position on the next save. The round-trip is the real assertion.
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

    // The invariant is "the target's column is in view": multi-column flow,
    // not scrollLeft, decides where inside the column the target sits.
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

  // `pageOffset` picks the column *within* a paragraph; `multipageSpec` wedges
  // a 300-sentence paragraph at HUGE so it spans columns at any viewport.
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

    // Wrapper must have finished growing before scrollLeft means anything.
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
