// The mock's rendering of a translated paragraph: what the backend's
// `render::paragraph_html` produces, for the specs that seed segments.
export type Mark = "emphasis" | "strong";

/** A translated paragraph as specs seed it: words, the gaps between, breaks. */
export type ParagraphSegment =
  | { kind: "gap"; text: string; marks?: Mark[] }
  | { kind: "break"; marks?: Mark[] }
  | {
      kind: "word";
      text: string;
      marks?: Mark[];
      sentence: number;
      word: number;
      flatIndex: number;
      translation: string | null;
    };

function escape(text: string, attribute: boolean): string {
  let out = text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
  if (attribute) out = out.replaceAll('"', "&quot;");
  return out;
}

export function paragraphHtml(segments: readonly ParagraphSegment[]): string {
  let html = "";
  for (const seg of segments) {
    if (seg.kind === "break") {
      html += "<br>";
      continue;
    }
    const strong = seg.marks?.includes("strong") ?? false;
    const emphasis = seg.marks?.includes("emphasis") ?? false;
    if (strong) html += "<strong>";
    if (emphasis) html += "<em>";
    if (seg.kind === "gap") {
      html += escape(seg.text, false);
    } else {
      const translation = seg.translation || null;
      html += `<span class="word-span" data-s="${seg.sentence}" data-w="${seg.word}" data-flat-index="${seg.flatIndex}"`;
      if (translation !== null)
        html += ` data-t="${escape(translation, true)}"`;
      html += ">";
      html += `${escape(seg.text, false)}</span>`;
    }
    if (emphasis) html += "</em>";
    if (strong) html += "</strong>";
  }
  return html;
}
