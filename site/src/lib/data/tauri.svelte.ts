import { invoke, type InvokeArgs } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type UpdateEvent<TEvent = any> = {
    name: string;
    filter: (ev: TEvent) => boolean;
};

const eventCleanupRegistry =
    typeof FinalizationRegistry !== "undefined"
        ? new FinalizationRegistry<UnlistenFn[]>((unlisteners) => {
              for (const u of unlisteners) {
                  try { u(); } catch { /* ignore */ }
              }
          })
        : null;

const pollerCleanupRegistry =
    typeof FinalizationRegistry !== "undefined"
        ? new FinalizationRegistry<ReturnType<typeof setInterval>>((id) => {
              clearInterval(id);
          })
        : null;

export class Resource<T> {
    #current: T | undefined = $state(undefined);

    constructor(
        getterName: string,
        args: InvokeArgs = {},
        events: UpdateEvent[] = [],
        defaultValue?: T,
    ) {
        this.#current = defaultValue;
        const unlisteners: UnlistenFn[] = [];

        const fetch = () => {
            invoke<T>(getterName, args).then((v) => {
                this.#current = v;
            });
        };

        for (const ev of events) {
            listen(ev.name, (event) => {
                if (ev.filter((event as any).payload)) fetch();
            }).then((u) => unlisteners.push(u));
        }
        fetch();

        eventCleanupRegistry?.register(this, unlisteners);
    }

    get current(): T | undefined {
        return this.#current;
    }
}

export class PollingResource<T> {
    #current: T | undefined = $state(undefined);

    constructor(getterName: string, args: InvokeArgs, intervalMs: number) {
        const tick = async () => {
            try {
                const v = await invoke<T | null>(getterName, args);
                this.#current = v ?? undefined;
            } catch { /* ignore */ }
        };
        const id = setInterval(tick, intervalMs);
        void tick();
        pollerCleanupRegistry?.register(this, id);
    }

    get current(): T | undefined {
        return this.#current;
    }
}
