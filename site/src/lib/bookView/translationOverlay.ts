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

type TranslationSizingContext = {
    metrics: OverlayMetrics | null;
    wordMetrics: WordMetrics | null;
    useDomWidth: boolean;
};

let metricsCache: OverlayMetrics | null = null;
let metricsCacheKey: string | null = null;
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

function withTranslationFontSizeCleared<T>(
    span: HTMLElement,
    fn: () => T,
): T {
    const prev = span.style.getPropertyValue(TRANSLATION_FONT_SIZE_VAR);
    if (prev) {
        span.style.removeProperty(TRANSLATION_FONT_SIZE_VAR);
    }
    try {
        return fn();
    } finally {
        if (prev) {
            span.style.setProperty(TRANSLATION_FONT_SIZE_VAR, prev);
        }
    }
}

function getMetrics(sampleSpan: HTMLElement): OverlayMetrics | null {
    return withTranslationFontSizeCleared(sampleSpan, () => {
        const styles = getComputedStyle(sampleSpan, "::before");
        const key = `${styles.font}|${styles.paddingLeft}|${styles.paddingRight}|${styles.borderLeftWidth}|${styles.borderRightWidth}|${styles.letterSpacing}|${styles.fontSize}`;
        if (metricsCache && metricsCacheKey === key) {
            return metricsCache;
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

        metricsCache = {
            font,
            baseFontSizePx,
            horizontalChromePx:
                paddingLeft + paddingRight + borderLeft + borderRight,
            letterSpacingPx,
        };
        metricsCacheKey = key;
        return metricsCache;
    });
}

function getWordMetrics(sampleSpan: HTMLElement): WordMetrics | null {
    const styles = getComputedStyle(sampleSpan);
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

function nextFrame(signal?: AbortSignal): Promise<void> {
    if (signal?.aborted) {
        return Promise.resolve();
    }
    return new Promise((resolve) => {
        if (typeof requestAnimationFrame === "function") {
            requestAnimationFrame(() => resolve());
        } else {
            setTimeout(resolve, 0);
        }
    });
}

function clearTranslationFontSize(span: HTMLElement) {
    if (span.style.getPropertyValue(TRANSLATION_FONT_SIZE_VAR)) {
        span.style.removeProperty(TRANSLATION_FONT_SIZE_VAR);
    }
}

function maybeSetTranslationFontSize(span: HTMLElement, value: string | null) {
    if (value === null) {
        clearTranslationFontSize(span);
        return;
    }

    if (span.style.getPropertyValue(TRANSLATION_FONT_SIZE_VAR) !== value) {
        span.style.setProperty(TRANSLATION_FONT_SIZE_VAR, value);
    }
}

function sizeTranslation(
    span: HTMLElement,
    translation: string,
    ctx: TranslationSizingContext,
): string | null {
    const metrics = ctx.metrics;
    const wordMetrics = ctx.wordMetrics;
    if (!metrics || !wordMetrics) {
        return null;
    }

    const parentWidth = ctx.useDomWidth
        ? span.getBoundingClientRect().width
        : measureTextWidthPx(span.textContent ?? "", wordMetrics);
    const availableWidth = parentWidth - metrics.horizontalChromePx - 0.5;
    if (availableWidth <= 0) {
        return null;
    }

    const textWidth = measureTextWidthPx(translation, metrics);
    if (textWidth <= availableWidth) {
        return null;
    }

    const scaledSizePx = metrics.baseFontSizePx * (availableWidth / textWidth);
    return `${scaledSizePx}px`;
}

function showAndSize(span: HTMLElement, ctx: TranslationSizingContext) {
    if (!span.classList.contains("show-translation")) {
        span.classList.add("show-translation");
    }

    const translation = span.dataset["translation"];
    if (!translation) {
        clearTranslationFontSize(span);
        return;
    }

    const sized = sizeTranslation(span, translation, ctx);
    maybeSetTranslationFontSize(span, sized);
}

export function showTranslation(span: HTMLElement) {
    showTranslations([span]);
}

export function showTranslations(spans: Iterable<HTMLElement>) {
    const items = Array.isArray(spans) ? spans : Array.from(spans);
    if (items.length === 0) {
        return;
    }

    const sample = items.find((span) => !!span.dataset["translation"]) ?? null;
    if (sample && !sample.classList.contains("show-translation")) {
        sample.classList.add("show-translation");
    }
    const ctx: TranslationSizingContext = {
        metrics: sample ? getMetrics(sample) : null,
        wordMetrics: sample ? getWordMetrics(sample) : null,
        useDomWidth: items.length <= 10,
    };

    for (const span of items) {
        showAndSize(span, ctx);
    }
}

export async function showTranslationsBatched(
    spans: Iterable<HTMLElement>,
    options: { batchSize?: number; signal?: AbortSignal } = {},
) {
    const items = Array.isArray(spans) ? spans : Array.from(spans);
    if (items.length === 0) {
        return;
    }

    const batchSize = Math.max(1, options.batchSize ?? 200);
    const signal = options.signal;

    const sample = items.find((span) => !!span.dataset["translation"]) ?? null;
    if (sample && !sample.classList.contains("show-translation")) {
        sample.classList.add("show-translation");
    }
    const ctx: TranslationSizingContext = {
        metrics: sample ? getMetrics(sample) : null,
        wordMetrics: sample ? getWordMetrics(sample) : null,
        useDomWidth: items.length <= 10,
    };

    for (let start = 0; start < items.length; start += batchSize) {
        if (signal?.aborted) {
            return;
        }

        const end = Math.min(items.length, start + batchSize);
        for (let i = start; i < end; i++) {
            const span = items[i];

            showAndSize(span, ctx);
        }

        await nextFrame(signal);
    }
}
