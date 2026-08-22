import { expect, test } from "../helpers/test";
import {
  expectTranslated,
  getTranslateCalls,
  paragraphLocator,
  seedAndOpen,
  translateButton,
  wordSegment,
  type ParagraphSegment,
} from "../helpers/paragraph";
import { getHarness } from "../../real/harness-registry";

/**
 * The app caches translation results by source text across runs (its cache dir
 * outlives the per-test config dir), and a cache hit makes no HTTP request at
 * all. Every paragraph here must therefore be textually unique per run.
 */
function texts(): { short: string; long: string; seeded: string } {
  const nonce = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
  return {
    // `short` is a substring of `long`: attribution must pick the longest
    // match, or every `long` translation is misreported as the `short` one.
    short: `Guten ${nonce}`,
    long: `Guten ${nonce} heute Morgen`,
    seeded: `Ein ${nonce} Satz.`,
  };
}

function segmentsOf(text: string): ParagraphSegment[] {
  return text.split(" ").map((token, i) =>
    wordSegment({
      flatIndex: i,
      sentence: 0,
      word: i,
      text: token,
      translation: `t${i}`,
    }),
  );
}

test.describe("real-tier helper contract", () => {
  test("getTranslateCalls reports only post-seed paragraph translations", async ({
    page,
  }) => {
    const t = texts();
    const { bookId } = await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            { html: t.short },
            { html: t.long },
            // Translated during the seed pass — its sim request must not
            // surface as a call, matching the mock's log.
            { html: t.seeded, segments: segmentsOf(t.seeded) },
          ],
        },
      ],
      translateConfigs: [
        {
          paragraphId: 1,
          cfg: { kind: "immediate", segments: segmentsOf(t.long) },
        },
      ],
    });

    expect(await getTranslateCalls(page)).toEqual([]);

    const p = paragraphLocator(page, 1);
    await translateButton(p).click();
    await expectTranslated(p);

    const calls = await getTranslateCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ bookId, paragraphId: 1, useCache: true });
  });

  test("summary requests are unary and paragraph translations stream", async ({
    page,
  }) => {
    // The discriminator getTranslateCalls relies on. Summary generation runs at
    // import and quotes every paragraph, so a path-blind filter would count it.
    const t = texts();
    await seedAndOpen(page, {
      chapters: [
        { paragraphs: [{ html: t.seeded, segments: segmentsOf(t.seeded) }] },
      ],
    });

    const paths = new Set(
      (await getHarness().llm.requests()).map((r) => r.path),
    );
    expect([...paths].some((p) => p.endsWith(":generateContent"))).toBe(true);
    expect([...paths].some((p) => p.endsWith(":streamGenerateContent"))).toBe(
      true,
    );
  });
});
