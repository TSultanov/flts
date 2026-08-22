import { describe, it, expect } from "vitest";
import { uint8ToBase64 } from "./uint8ToBase64";

function roundtrip(bytes: Uint8Array): Uint8Array {
  const binary = atob(uint8ToBase64(bytes));
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

describe("uint8ToBase64", () => {
  it("encodes an empty buffer as an empty string", () => {
    expect(uint8ToBase64(new Uint8Array())).toBe("");
  });

  it("encodes ASCII bytes as standard base64", () => {
    expect(uint8ToBase64(new TextEncoder().encode("hello"))).toBe("aGVsbG8=");
  });

  it("uses standard alphabet, not base64url", () => {
    // 0xfb 0xff → '+/8=' in STANDARD, '-_8=' in url-safe.
    expect(uint8ToBase64(new Uint8Array([0xfb, 0xff]))).toBe("+/8=");
  });

  it("round-trips buffers larger than the 0x8000 fromCharCode chunk", () => {
    const bytes = new Uint8Array(0x8000 + 17);
    for (let i = 0; i < bytes.length; i++) bytes[i] = i & 0xff;
    expect(roundtrip(bytes)).toEqual(bytes);
  });
});
