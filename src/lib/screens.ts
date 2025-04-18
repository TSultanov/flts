import type { Book } from "./library"

export type ReaderState = {
    book: Book
}
export type ScreenState = "Library" | "Config" | ReaderState