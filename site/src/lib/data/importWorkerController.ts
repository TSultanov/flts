import type { Library } from "../library.svelte";
import type { ParagraphTranslatedResponse } from "./importWorker";
import ImportWorker from "./importWorker?worker";

export class ImportWorkerController {
    private worker: Worker;
    private onParagraphTranslatedHandlers: Array<(paragraphId: number) => Promise<void>> = [];

    constructor() {
        this.worker = new ImportWorker();

        this.worker.addEventListener("message", async (msg: MessageEvent<ParagraphTranslatedResponse>) => {
            switch (msg.data?.__brand) {
                case 'ParagraphTranslatedResponse': {
                    await Promise.all(this.onParagraphTranslatedHandlers.map(p => p(msg.data.paragraphId)));
                    break;
                }
                default: {
                    break;
                }
            }
        });
    }

    addOnParagraphTranslatedHandler(handler: () => Promise<void>) {
        this.onParagraphTranslatedHandlers.push(handler);
    }
}