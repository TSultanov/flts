export function debounce(callbackFn: () => void | Promise<void>, timeout: number) {
    let timeoutId: NodeJS.Timeout | null = null;
    let lastCallTime = 0;

    return () => {
        const now = Date.now();
        if (now - lastCallTime >= timeout) {
            lastCallTime = now;
            callbackFn();
            if (timeoutId) {
                clearTimeout(timeoutId);
                timeoutId = null;
            }
            return;
        }

        if (timeoutId) {
            clearTimeout(timeoutId);
            timeoutId = null;
        }

        timeoutId = setTimeout(() => {
            lastCallTime = Date.now(),
            callbackFn();
            timeoutId = null;
        }, timeout);
    }
}