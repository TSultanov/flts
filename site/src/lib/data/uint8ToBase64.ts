/** STANDARD base64 (not url-safe). Chunked so large buffers do not overflow the stack. */
export function uint8ToBase64(bytes: Uint8Array): string {
    let binary = "";
    const step = 0x8000;
    for (let i = 0; i < bytes.length; i += step) {
        binary += String.fromCharCode(...bytes.subarray(i, i + step));
    }
    return btoa(binary);
}
