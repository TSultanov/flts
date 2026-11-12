// A translated paragraph arrives as HTML. This keeps on it what only the page
// knows: which word is selected, which were tapped to show their translation,
// and the fitting of each shown translation to its word.
import { anchorOverlay, sizeOverlay } from "./translationOverlay";

export type TranslatedParagraph = {
  html: string;
  /** The selected word, if it is in this paragraph. */
  selected: { sentence: number; word: number } | null;
  /** Flat indices of the words tapped to show their translation. */
  revealed: readonly number[];
};

export function translatedParagraph(
  wrapper: HTMLElement,
  initial: TranslatedParagraph,
) {
  const p = wrapper.querySelector("p")!;
  let html: string | null = null;
  // Each shown translation, and how to let go of its fitting.
  const fitted = new Map<Element, () => void>();

  const release = () => {
    for (const let_go of fitted.values()) let_go();
    fitted.clear();
  };

  const fit = (overlay: HTMLElement) => {
    const span = overlay.parentElement as HTMLElement;
    const text = span.lastChild?.textContent ?? "";
    const translation = overlay.textContent ?? "";
    sizeOverlay(span, overlay, text, translation);
    const let_go = anchorOverlay(span, overlay, text, translation);
    fitted.set(overlay, () => {
      let_go();
      span.style.removeProperty("--word-translation-font-size");
    });
  };

  const apply = (now: TranslatedParagraph) => {
    if (now.html !== html) {
      release();
      html = now.html;
      p.innerHTML = html;
    }
    for (const flat of now.revealed) {
      const span = p.querySelector<HTMLElement>(
        `.word-span[data-flat-index="${flat}"]`,
      );
      const translation = span?.dataset["t"];
      if (!span || !translation) continue;
      if (span.firstElementChild?.classList.contains("translation-overlay")) {
        continue;
      }
      const overlay = document.createElement("span");
      overlay.className = "translation-overlay tapped";
      overlay.setAttribute("aria-hidden", "true");
      overlay.textContent = translation;
      span.prepend(overlay);
      fit(overlay);
    }
    const selected = now.selected
      ? p.querySelector(
          `.word-span[data-s="${now.selected.sentence}"][data-w="${now.selected.word}"]`,
        )
      : null;
    for (const was of p.querySelectorAll(".word-span.selected")) {
      if (was !== selected) was.classList.remove("selected");
    }
    selected?.classList.add("selected");
  };

  apply(initial);
  return { update: apply, destroy: release };
}
