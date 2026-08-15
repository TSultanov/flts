export function suggestSourceLanguage(
    parsedId: string | null | undefined,
    availableIds: readonly string[],
    fallback = "eng",
): string {
    if (!parsedId) {
        return fallback;
    }
    if (availableIds.length === 0 || availableIds.includes(parsedId)) {
        return parsedId;
    }
    return fallback;
}
