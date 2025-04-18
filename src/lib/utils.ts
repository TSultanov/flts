import { sha256 } from 'js-sha256';

export async function hashBuffer(data: ArrayBuffer | Uint8Array) {
    return sha256(data);
}

export async function hashString(str: string) {
    return await hashBuffer(new TextEncoder().encode(str))
}

export async function hashFile(file: File) {
    let content = await file.arrayBuffer();
    return await hashBuffer(content);
}