import { mount, unmount } from "svelte";
import Overlay from "./overlay/Overlay.svelte";
import { getSentences, getWords } from "@/lib/utils";
import './popup/app.css';

let overlay: Record<string, any> | null = null;
let container: HTMLElement | null = null;

let ui: globalThis.ShadowRootContentScriptUi<void> | null = null;
let props: { x: number, y: number, position: number, word: string; sentence: string, paragraph: string; onClose: () => void } | null = null;
let overlayTimeout: string | number | NodeJS.Timeout | null | undefined = null;
let callOverlayDelegate: (() => void) | null = null;

function showOverlay({ x, y }: { x: number, y: number }, position: number, word: string, sentence: string, paragraph: string) {
  hideOverlay();

  if (ui) {
    props = {
      x, y,
      position,
      word,
      sentence,
      paragraph,
      onClose: hideOverlay
    };
    ui.uiContainer.style.position = 'absolute';
    ui.uiContainer.style.top = `0px`;
    ui.uiContainer.style.left = `0px`;

    ui.mount();
  }
}

function hideOverlay() {
  ui?.remove();
}


export default defineContentScript({
  matches: ['*://*/*'],
  cssInjectionMode: 'ui',
  async main(ctx) {
    ui = await createShadowRootUi(ctx, {
      name: 'example-ui',
      position: 'inline',
      anchor: 'body',
      onMount(container) {
        // Define how your UI will be mounted inside the container
        const target = document.createElement('div');
        container.append(target);

        if (props) {
          mount(Overlay, {
            target,
            props
          })
        }
      },
    });

    document.addEventListener('mouseup', (e: Event) => {
      if (callOverlayDelegate) {
        callOverlayDelegate();
        callOverlayDelegate = null;
      }
    });

    document.addEventListener('selectionchange', (e: Event) => {
      if (callOverlayDelegate) {
        callOverlayDelegate = null;
      }
      if (overlayTimeout) {
        clearTimeout(overlayTimeout);
      }

      callOverlayDelegate = () => {

        const selection = document.getSelection();
        const selectedText = selection?.toString();
        if (selection && selectedText && selectedText.length > 0) {
          overlayTimeout = setTimeout(() => {
            const firstRange = selection.getRangeAt(0);
            const rect = firstRange?.getBoundingClientRect();
            let paragraphNode = document.getSelection()?.focusNode;
            if (paragraphNode && paragraphNode.nodeType === Node.TEXT_NODE && paragraphNode.parentNode && paragraphNode.parentNode.nodeType !== Node.TEXT_NODE) {
              paragraphNode = paragraphNode.parentNode;
            }

            let paragraphText = paragraphNode?.textContent || "";
            let sentence = paragraphNode?.textContent || "";
            let position = 0;

            if (paragraphNode) {
              const sentences = getSentences(paragraphNode);
              for (const s of sentences) {
                const s2s = s.compareBoundaryPoints(Range.START_TO_START, firstRange);
                const e2e = s.compareBoundaryPoints(Range.END_TO_END, firstRange);
                const a = s2s <= 0;
                const b = e2e >= 0;
                const contains = a && b;
                if (contains) {
                  sentence = s.toString();
                  position = 0;
                  for (const w of getWords(s)) {
                    const s2s = w.compareBoundaryPoints(Range.START_TO_START, firstRange);
                    if (s2s >= 0) {
                      position = position;
                      break;
                    }
                    position++;
                  }
                  break;
                }
              }
            }

            console.log(rect);
            showOverlay({ x: rect?.left || 0, y: rect?.bottom || 0 }, position, selectedText, sentence, paragraphText);
          }, 200);
        } else {
          hideOverlay();
        }
      }
    })
  },
});
