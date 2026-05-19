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
        horizontalChromePx:
            paddingLeft + paddingRight + borderLeft + borderRight,
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

export function sizeOverlay(
    span: HTMLElement,
    overlay: HTMLElement,
    wordText: string,
    translationText: string,
): void {
    const overlayMetrics = getOverlayMetrics(overlay);
    const wordMetrics = getWordMetrics(span);
    if (!overlayMetrics || !wordMetrics) {
        return;
    }

    const parentWidthPx = measureTextWidthPx(wordText, wordMetrics);
    const availableWidthPx = parentWidthPx - overlayMetrics.horizontalChromePx - 0.5;
    if (availableWidthPx <= 0) {
        return;
    }

    const textWidthPx = measureTextWidthPx(translationText, overlayMetrics);
    if (textWidthPx <= availableWidthPx) {
        span.style.removeProperty(TRANSLATION_FONT_SIZE_VAR);
        return;
    }

    const scaledPx = overlayMetrics.baseFontSizePx * (availableWidthPx / textWidthPx);
    span.style.setProperty(TRANSLATION_FONT_SIZE_VAR, `${scaledPx}px`);
}
