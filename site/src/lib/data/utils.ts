import { sha256 } from 'js-sha256';

export async function hashBuffer(data: ArrayBuffer | Uint8Array): Promise<string> {
    let result: string;

    if (typeof crypto !== 'undefined' && crypto.subtle) {
        try {
            const hashBuffer = await crypto.subtle.digest('SHA-256', data);
            const hashArray = Array.from(new Uint8Array(hashBuffer));
            result = hashArray.map(b => b.toString(16).padStart(2, '0')).join('');
        } catch (error) {
            console.warn("SubtleCrypto failed, falling back to js-sha256:", error);
            // Fallback to js-sha256 if SubtleCrypto fails for some reason
            result = sha256(data);
        }
    } else {
        // Fallback to js-sha256 if SubtleCrypto is not available
        result = sha256(data);
    }
    return result;
}

export async function hashString(str: string) {
    return await hashBuffer(new TextEncoder().encode(str))
}

export async function hashFile(file: File) {
    const content = await file.arrayBuffer();
    return await hashBuffer(content);
}
