type OverlayMetrics = {
  font: string;
  baseFontSizePx: number;
  horizontalChromePx: number;
  letterSpacingPx: number;
};

type WordMetrics = {
  font: string;
  baseFontSizePx: number;
  letterSpacingPx: number;
};

let overlayMetricsCache: OverlayMetrics | null = null;
let overlayMetricsCacheKey: string | null = null;
let wordMetricsCache: WordMetrics | null = null;
let wordMetricsCacheKey: string | null = null;

const TRANSLATION_FONT_SIZE_VAR = "--word-translation-font-size";
const MAX_TEXT_WIDTH_CACHE_ENTRIES = 5000;

const textWidthCache = new Map<string, number>();
const context = (() => {
  if (typeof document === "undefined") {
    return null;
  }
  const canvas = document.createElement("canvas");
  return canvas.getContext("2d");
})();

function getOverlayMetrics(overlay: HTMLElement): OverlayMetrics | null {
  const styles = getComputedStyle(overlay);
  const key = `${styles.font}|${styles.paddingLeft}|${styles.paddingRight}|${styles.borderLeftWidth}|${styles.borderRightWidth}|${styles.letterSpacing}|${styles.fontSize}`;
  if (overlayMetricsCache && overlayMetricsCacheKey === key) {
    return overlayMetricsCache;
  }

  const paddingLeft = parseFloat(styles.paddingLeft) || 0;
  const paddingRight = parseFloat(styles.paddingRight) || 0;
  const borderLeft = parseFloat(styles.borderLeftWidth) || 0;
  const borderRight = parseFloat(styles.borderRightWidth) || 0;

  const baseFontSizePx = parseFloat(styles.fontSize);
  if (!baseFontSizePx || Number.isNaN(baseFontSizePx)) {
    return null;
  }

  const font =
    styles.font ||
    `${styles.fontStyle} ${styles.fontVariant} ${styles.fontWeight} ${styles.fontSize}/${styles.lineHeight} ${styles.fontFamily}`;

  const letterSpacingPx =
    styles.letterSpacing === "normal"
      ? 0
      : parseFloat(styles.letterSpacing) || 0;

  overlayMetricsCache = {
    font,
    baseFontSizePx,
    horizontalChromePx: paddingLeft + paddingRight + borderLeft + borderRight,
    letterSpacingPx,
  };
  overlayMetricsCacheKey = key;
  return overlayMetricsCache;
}

function getWordMetrics(span: HTMLElement): WordMetrics | null {
  const styles = getComputedStyle(span);
  const key = `${styles.font}|${styles.letterSpacing}|${styles.fontSize}`;
  if (wordMetricsCache && wordMetricsCacheKey === key) {
    return wordMetricsCache;
  }

  const baseFontSizePx = parseFloat(styles.fontSize);
  if (!baseFontSizePx || Number.isNaN(baseFontSizePx)) {
    return null;
  }

  const font =
    styles.font ||
    `${styles.fontStyle} ${styles.fontVariant} ${styles.fontWeight} ${styles.fontSize}/${styles.lineHeight} ${styles.fontFamily}`;

  const letterSpacingPx =
    styles.letterSpacing === "normal"
      ? 0
      : parseFloat(styles.letterSpacing) || 0;

  wordMetricsCache = {
    font,
    baseFontSizePx,
    letterSpacingPx,
  };
  wordMetricsCacheKey = key;
  return wordMetricsCache;
}

function measureTextWidthPx(
  text: string,
  metrics: { font: string; baseFontSizePx: number; letterSpacingPx: number },
): number {
  if (!context) {
    return text.length * metrics.baseFontSizePx;
  }

  const cacheKey = `${metrics.font}\0${metrics.letterSpacingPx}\0${text}`;
  const cached = textWidthCache.get(cacheKey);
  if (cached !== undefined) {
    return cached;
  }

  context.font = metrics.font;
  let width = context.measureText(text).width;
  if (metrics.letterSpacingPx !== 0 && text.length > 1) {
    width += metrics.letterSpacingPx * (text.length - 1);
  }

  if (textWidthCache.size >= MAX_TEXT_WIDTH_CACHE_ENTRIES) {
    textWidthCache.clear();
  }
  textWidthCache.set(cacheKey, width);
  return width;
}

function applyFit(
  span: HTMLElement,
  overlay: HTMLElement,
  wordWidthPx: number,
  translationText: string,
): void {
  const overlayMetrics = getOverlayMetrics(overlay);
  if (!overlayMetrics) {
    return;
  }

  const availableWidthPx =
    wordWidthPx - overlayMetrics.horizontalChromePx - 0.5;
  if (availableWidthPx <= 0) {
    return;
  }

  const textWidthPx = measureTextWidthPx(translationText, overlayMetrics);
  if (textWidthPx <= availableWidthPx) {
    span.style.removeProperty(TRANSLATION_FONT_SIZE_VAR);
    return;
  }

  const scaledPx =
    overlayMetrics.baseFontSizePx * (availableWidthPx / textWidthPx);
  span.style.setProperty(TRANSLATION_FONT_SIZE_VAR, `${scaledPx}px`);
}

export function sizeOverlay(
  span: HTMLElement,
  overlay: HTMLElement,
  wordText: string,
  translationText: string,
): void {
  const wordMetrics = getWordMetrics(span);
  if (!wordMetrics) {
    return;
  }
  applyFit(
    span,
    overlay,
    measureTextWidthPx(wordText, wordMetrics),
    translationText,
  );
}

type Anchor = {
  span: HTMLElement;
  overlay: HTMLElement;
  wordText: string;
  translationText: string;
};

const anchors = new Set<Anchor>();
let anchorFrame: number | null = null;
let resizeBound = false;

function scheduleAnchorPass(): void {
  if (anchorFrame !== null || typeof requestAnimationFrame === "undefined") {
    return;
  }
  anchorFrame = requestAnimationFrame(runAnchorPass);
}

/** Reads all fragment boxes before it writes a style: one layout per pass. */
function runAnchorPass(): void {
  anchorFrame = null;
  const writes: Array<[Anchor, DOMRect[] | null]> = [];
  for (const anchor of anchors) {
    const rects = Array.from(anchor.span.getClientRects());
    writes.push([anchor, rects.length > 1 ? rects : null]);
  }
  for (const [anchor, rects] of writes) {
    if (!rects) {
      releaseAnchor(anchor);
      continue;
    }
    // The containing block of a fragmented inline runs from the first
    // fragment's start to the last one's end. That is backwards across a line
    // break, so `width: 100%` gives nothing. Pin the widest fragment instead.
    const widest = rects.reduce((a, b) => (b.width > a.width ? b : a));
    const origin = rects[0];
    anchor.overlay.style.left = `${widest.left - origin.left}px`;
    anchor.overlay.style.top = `${widest.top - origin.top}px`;
    anchor.overlay.style.right = "auto";
    anchor.overlay.style.width = `${widest.width}px`;
    applyFit(anchor.span, anchor.overlay, widest.width, anchor.translationText);
  }
}

function releaseAnchor(anchor: Anchor): void {
  const { style } = anchor.overlay;
  if (style.width === "") {
    return;
  }
  style.removeProperty("left");
  style.removeProperty("top");
  style.removeProperty("right");
  style.removeProperty("width");
  sizeOverlay(
    anchor.span,
    anchor.overlay,
    anchor.wordText,
    anchor.translationText,
  );
}

function onResize(): void {
  scheduleAnchorPass();
}

/** Keeps the overlay over a word that hyphenates. Call the result to release. */
export function anchorOverlay(
  span: HTMLElement,
  overlay: HTMLElement,
  wordText: string,
  translationText: string,
): () => void {
  const anchor: Anchor = { span, overlay, wordText, translationText };
  anchors.add(anchor);
  if (!resizeBound && typeof window !== "undefined") {
    window.addEventListener("resize", onResize);
    resizeBound = true;
  }
  scheduleAnchorPass();
  return () => {
    anchors.delete(anchor);
  };
}
