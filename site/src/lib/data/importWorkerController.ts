import type { ParagraphTranslatedResponse, ScheduleTranslationRequest } from "./importWorker";
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
                    // We want to make worker to send messages to itself, so anything we don't know how to handle we reflect back
                    this.worker.postMessage(msg.data);
                    break;
                }
            }
        });
    }

    addOnParagraphTranslatedHandler(handler: () => Promise<void>) {
        this.onParagraphTranslatedHandlers.push(handler);
    }

    startScheduling() {
        const message: ScheduleTranslationRequest = {
            __brand: "ScheduleTranslationRequest"
        }
        this.worker.postMessage(message);
    }
}