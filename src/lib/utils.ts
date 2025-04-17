export async function hashBuffer(data: BufferSource) {
    let digest = await crypto.subtle.digest('SHA-256', data);
    let hexes = [],
    view = new DataView(digest);
    for (let i = 0; i < view.byteLength; i += 4) {
        hexes.push(('00000000' + view.getUint32(i).toString(16)).slice(-8));
    }
    return hexes.join('');
}

export async function hashString(str: string) {
    return await hashBuffer(new TextEncoder().encode(str))
}

export async function hashFile(file: File) {
    let content = await file.bytes();
    return await hashBuffer(content);
}