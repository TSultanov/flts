import { describe, it, expect } from "vitest";
import {
  SHARED_IDENTITY_MESSAGE,
  sharesIdentity,
  storeIssueMessages,
} from "./syncStatusHelpers";
import type { StoreIssues, SyncStatus } from "./store.svelte";

const NEWER =
  "Another device uses a newer version of the app. Update this device to see its latest changes.";
const UNREADABLE =
  "Some synced data could not be read. Changes from another device may be missing.";
const STRANGERS =
  "The sync folder contains conflict copies that are being ignored.";

function status(storeIssues?: Partial<StoreIssues>): SyncStatus {
  const base: SyncStatus = {
    state: "online",
    deviceCount: 1,
    connectedCount: 1,
  };
  if (!storeIssues) return base;
  return {
    ...base,
    storeIssues: {
      newerVersion: 0,
      unreadable: 0,
      strangers: 0,
      ...storeIssues,
    },
  };
}

describe("storeIssueMessages", () => {
  it("returns nothing for an undefined status", () => {
    expect(storeIssueMessages(undefined)).toEqual([]);
  });
  it("returns nothing when the field is absent", () => {
    expect(storeIssueMessages(status())).toEqual([]);
  });
  it("returns nothing when all counts are zero", () => {
    expect(storeIssueMessages(status({}))).toEqual([]);
  });
  it("warns about a newer version", () => {
    expect(storeIssueMessages(status({ newerVersion: 1 }))).toEqual([NEWER]);
  });
  it("warns about unreadable data", () => {
    expect(storeIssueMessages(status({ unreadable: 3 }))).toEqual([UNREADABLE]);
  });
  it("treats rows that stay unapplied as a newer version elsewhere", () => {
    expect(storeIssueMessages(status({ stalled: true }))).toEqual([NEWER]);
  });
  it("leaves a shared identity to its own notice", () => {
    expect(storeIssueMessages(status({ sharedIdentity: true }))).toEqual([]);
    expect(
      storeIssueMessages(status({ sharedIdentity: true, unreadable: 1 })),
    ).toEqual([UNREADABLE]);
  });
  it("warns about stranger files", () => {
    expect(storeIssueMessages(status({ strangers: 2 }))).toEqual([STRANGERS]);
  });
  it("lists every non-zero issue in a fixed order", () => {
    expect(
      storeIssueMessages(
        status({ newerVersion: 1, unreadable: 1, strangers: 1 }),
      ),
    ).toEqual([NEWER, UNREADABLE, STRANGERS]);
  });
});

describe("sharesIdentity", () => {
  it("is true when the flag is set", () => {
    expect(sharesIdentity(status({ sharedIdentity: true }))).toBe(true);
  });
  it("is false when the flag is cleared", () => {
    expect(sharesIdentity(status({ sharedIdentity: false }))).toBe(false);
  });
  it("is false when the flag is absent", () => {
    expect(sharesIdentity(status({}))).toBe(false);
  });
  it("is false without store issues or a status", () => {
    expect(sharesIdentity(status())).toBe(false);
    expect(sharesIdentity(undefined)).toBe(false);
  });
  it("words the notice", () => {
    expect(SHARED_IDENTITY_MESSAGE).toBe(
      "This device shares its sync identity with another copy of the app, as happens after restoring a backup. One of the two needs a new identity and will then have to be paired again.",
    );
  });
});
