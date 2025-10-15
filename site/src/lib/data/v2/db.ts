import { v4 as uuidv4 } from "uuid";
export type UUID = string & { readonly __brand: "UUID" };

function isValidUUID(value: string): value is UUID {
    const uuidRegex = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
    return uuidRegex.test(value);
}

export function createUUID(value: string): UUID {
    if (!isValidUUID(value)) {
        throw new Error(`Invalid UUID format: ${value}`);
    }
    return value as UUID;
}

export function generateUID(): UUID {
    return createUUID(uuidv4());
}
