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

function getMetrics(sampleSpan: HTMLElement): OverlayMetrics | null {
    const prevFontSize = sampleSpan.style.getPropertyValue(
        TRANSLATION_FONT_SIZE_VAR,
    );
    if (prevFontSize) {
        sampleSpan.style.removeProperty(TRANSLATION_FONT_SIZE_VAR);
    }
    const styles = getComputedStyle(sampleSpan, "::before");
    const key = `${styles.font}|${styles.paddingLeft}|${styles.paddingRight}|${styles.borderLeftWidth}|${styles.borderRightWidth}|${styles.letterSpacing}|${styles.fontSize}`;
    if (metricsCache && metricsCacheKey === key) {
        if (prevFontSize) {
            sampleSpan.style.setProperty(TRANSLATION_FONT_SIZE_VAR, prevFontSize);
        }
        return metricsCache;
    }

    const paddingLeft = parseFloat(styles.paddingLeft) || 0;
    const paddingRight = parseFloat(styles.paddingRight) || 0;
    const borderLeft = parseFloat(styles.borderLeftWidth) || 0;
    const borderRight = parseFloat(styles.borderRightWidth) || 0;

    const baseFontSizePx = parseFloat(styles.fontSize);
    if (!baseFontSizePx || Number.isNaN(baseFontSizePx)) {
        if (prevFontSize) {
            sampleSpan.style.setProperty(TRANSLATION_FONT_SIZE_VAR, prevFontSize);
        }
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
        horizontalChromePx: paddingLeft + paddingRight + borderLeft + borderRight,
        letterSpacingPx,
    };
    metricsCacheKey = key;
    if (prevFontSize) {
        sampleSpan.style.setProperty(TRANSLATION_FONT_SIZE_VAR, prevFontSize);
    }
    return metricsCache;
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

export function showTranslation(span: HTMLElement) {
    showTranslations([span]);
}

export function showTranslations(spans: Iterable<HTMLElement>) {
    const items = Array.isArray(spans) ? spans : Array.from(spans);
    if (items.length === 0) {
        return;
    }

    const sample = items.find((span) => !!span.dataset["translation"]) ?? null;
    const metrics = sample ? getMetrics(sample) : null;
    const wordMetrics = sample ? getWordMetrics(sample) : null;

    const plans: Array<{
        span: HTMLElement;
        desiredFontSizePx: number | null;
    }> = [];

    if (metrics && wordMetrics) {
        const useDomWidth = items.length <= 10;
        for (const span of items) {
            const translation = span.dataset["translation"];
            if (!translation) {
                continue;
            }

            const parentWidth = useDomWidth
                ? span.getBoundingClientRect().width
                : measureTextWidthPx(span.textContent ?? "", wordMetrics);
            const availableWidth =
                parentWidth - metrics.horizontalChromePx - 0.5;
            if (availableWidth <= 0) {
                plans.push({ span, desiredFontSizePx: null });
                continue;
            }

            const textWidth = measureTextWidthPx(translation, metrics);
            if (textWidth <= availableWidth) {
                plans.push({ span, desiredFontSizePx: null });
                continue;
            }

            const scaledSizePx =
                metrics.baseFontSizePx * (availableWidth / textWidth);
            plans.push({ span, desiredFontSizePx: scaledSizePx });
        }
    }

    for (const span of items) {
        if (!span.classList.contains("show-translation")) {
            span.classList.add("show-translation");
        }
    }

    if (!metrics) {
        for (const span of items) {
            if (span.style.getPropertyValue(TRANSLATION_FONT_SIZE_VAR)) {
                span.style.removeProperty(TRANSLATION_FONT_SIZE_VAR);
            }
        }
        return;
    }

    for (const { span, desiredFontSizePx } of plans) {
        if (desiredFontSizePx == null) {
            if (span.style.getPropertyValue(TRANSLATION_FONT_SIZE_VAR)) {
                span.style.removeProperty(TRANSLATION_FONT_SIZE_VAR);
            }
            continue;
        }

        const value = `${desiredFontSizePx}px`;
        if (span.style.getPropertyValue(TRANSLATION_FONT_SIZE_VAR) !== value) {
            span.style.setProperty(TRANSLATION_FONT_SIZE_VAR, value);
        }
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
    const metrics = sample ? getMetrics(sample) : null;
    const wordMetrics = sample ? getWordMetrics(sample) : null;
    const useDomWidth = items.length <= 10;

    for (let start = 0; start < items.length; start += batchSize) {
        if (signal?.aborted) {
            return;
        }

        const end = Math.min(items.length, start + batchSize);
        for (let i = start; i < end; i++) {
            const span = items[i];

            if (!span.classList.contains("show-translation")) {
                span.classList.add("show-translation");
            }

            const translation = span.dataset["translation"];
            if (!translation || !metrics || !wordMetrics) {
                if (span.style.getPropertyValue(TRANSLATION_FONT_SIZE_VAR)) {
                    span.style.removeProperty(TRANSLATION_FONT_SIZE_VAR);
                }
                continue;
            }

            const parentWidth = useDomWidth
                ? span.getBoundingClientRect().width
                : measureTextWidthPx(span.textContent ?? "", wordMetrics);
            const availableWidth = parentWidth - metrics.horizontalChromePx - 0.5;
            if (availableWidth <= 0) {
                if (span.style.getPropertyValue(TRANSLATION_FONT_SIZE_VAR)) {
                    span.style.removeProperty(TRANSLATION_FONT_SIZE_VAR);
                }
                continue;
            }

            const textWidth = measureTextWidthPx(translation, metrics);
            if (textWidth <= availableWidth) {
                if (span.style.getPropertyValue(TRANSLATION_FONT_SIZE_VAR)) {
                    span.style.removeProperty(TRANSLATION_FONT_SIZE_VAR);
                }
                continue;
            }

            const scaledSizePx =
                metrics.baseFontSizePx * (availableWidth / textWidth);
            const value = `${scaledSizePx}px`;
            if (span.style.getPropertyValue(TRANSLATION_FONT_SIZE_VAR) !== value) {
                span.style.setProperty(TRANSLATION_FONT_SIZE_VAR, value);
            }
        }

        await nextFrame(signal);
    }
}
