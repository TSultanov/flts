import {
  type SeedParagraph,
  type ParagraphSegment,
} from "../e2e/helpers/paragraph";

type Word = { text: string; translation: string };
type Token = Word | string; // Strings are literal gaps between word objects.
type Input = { raw: string; translated: Token[] };
const w = (text: string, translation: string): Word => ({ text, translation });

const proseSource =
  "Una mañana tranquila apareció una viajera cerca del jardín antiguo. Después de caminar durante muchas horas encontró una biblioteca extraordinariamente silenciosa. Las ventanas estaban abiertas y dejaban entrar una brisa agradable mientras alguien buscaba cuidadosamente una pequeña libreta entre los libros de aquella habitación luminosa.".split(
    " ",
  );
const proseTranslation = [
  w("Una", "A"),
  w("mañana", "morning"),
  w("tranquila", "calm"),
  w("apareció", "appeared"),
  w("una", "a"),
  w("viajera", "traveler"),
  w("cerca", "near"),
  w("del", "of the"),
  w("jardín", "garden"),
  w("antiguo.", "old"),
  w("Después", "After"),
  w("de", "of"),
  w("caminar", "walking"),
  w("durante", "during"),
  w("muchas", "many"),
  w("horas", "hours"),
  w("encontró", "found"),
  w("una", "a"),
  w("biblioteca", "library"),
  w("extraordinariamente", "extraordinarily"),
  w("silenciosa.", "quiet"),
  w("Las", "The"),
  w("ventanas", "windows"),
  w("estaban", "were"),
  w("abiertas", "open"),
  w("y", "and"),
  w("dejaban", "let"),
  w("entrar", "enter"),
  w("una", "a"),
  w("brisa", "breeze"),
  w("agradable", "pleasant"),
  w("mientras", "while"),
  w("alguien", "someone"),
  w("buscaba", "looked for"),
  w("cuidadosamente", "carefully"),
  w("una", "a"),
  w("pequeña", "small"),
  w("libreta", "notebook"),
  w("entre", "among"),
  w("los", "the"),
  w("libros", "books"),
  w("de", "of"),
  w("aquella", "that"),
  w("habitación", "room"),
  w("luminosa.", "bright"),
];

const mixed: Input = {
  raw: "«sí»… (a/b), A.V. jardín; histórico-cultural, té. —dijo—. l’homme, ¡ah!, (a/b), (quizá), V.A., ...¿qué? té. ex–presidente… l’homme, «sí»… (a/b), (quizá), ¿¡cómo!? ¿biblioteca? té. —dijo—. l’homme, “lectura”, teórico/práctico: tarde: V.A., ¿biblioteca? ¿dónde?, —¿Cómo? l’homme, ¡ah!, (a/b), (quizá), jardín; histórico-cultural, té. ex–presidente… ¡extraordinariamente! “lectura”, (a/b), A.V. V.A., ¿biblioteca? té. —¿Cómo? biblioteca(s) “lectura”, (a/b), tarde:",
  translated: [
    "«",
    w("sí", "yes"),
    "»… (",
    w("a", "a"),
    "/",
    w("b", "b"),
    "), ",
    w("A", "A"),
    ".",
    w("V", "V"),
    ". ",
    w("jardín", "garden"),
    "; ",
    w("histórico", "historical"),
    "-",
    w("cultural", "cultural"),
    ", ",
    w("té", "tea"),
    ". —",
    w("dijo", "said"),
    "—. ",
    w("l", "the"),
    "’",
    w("homme", "man"),
    ", ¡",
    w("ah", "ah"),
    "!, (",
    w("a", "a"),
    "/",
    w("b", "b"),
    "), (",
    w("quizá", "perhaps"),
    "), ",
    w("V", "V"),
    ".",
    w("A", "A"),
    "., ...¿",
    w("qué", "what"),
    "? ",
    w("té", "tea"),
    ". ",
    w("ex", "former"),
    "–",
    w("presidente", "president"),
    "… ",
    w("l", "the"),
    "’",
    w("homme", "man"),
    ", «",
    w("sí", "yes"),
    "»… (",
    w("a", "a"),
    "/",
    w("b", "b"),
    "), (",
    w("quizá", "perhaps"),
    "), ¿¡",
    w("cómo", "how"),
    "!? ¿",
    w("biblioteca", "library"),
    "? ",
    w("té", "tea"),
    ". —",
    w("dijo", "said"),
    "—. ",
    w("l", "the"),
    "’",
    w("homme", "man"),
    ", “",
    w("lectura", "reading"),
    "”, ",
    w("teórico", "theoretical"),
    "/",
    w("práctico", "practical"),
    ": ",
    w("tarde", "afternoon"),
    ": ",
    w("V", "V"),
    ".",
    w("A", "A"),
    "., ¿",
    w("biblioteca", "library"),
    "? ¿",
    w("dónde", "where"),
    "?, —¿",
    w("Cómo", "how"),
    "? ",
    w("l", "the"),
    "’",
    w("homme", "man"),
    ", ¡",
    w("ah", "ah"),
    "!, (",
    w("a", "a"),
    "/",
    w("b", "b"),
    "), (",
    w("quizá", "perhaps"),
    "), ",
    w("jardín", "garden"),
    "; ",
    w("histórico", "historical"),
    "-",
    w("cultural", "cultural"),
    ", ",
    w("té", "tea"),
    ". ",
    w("ex", "former"),
    "–",
    w("presidente", "president"),
    "… ¡",
    w("extraordinariamente", "extraordinarily"),
    "! “",
    w("lectura", "reading"),
    "”, (",
    w("a", "a"),
    "/",
    w("b", "b"),
    "), ",
    w("A", "A"),
    ".",
    w("V", "V"),
    ". ",
    w("V", "V"),
    ".",
    w("A", "A"),
    "., ¿",
    w("biblioteca", "library"),
    "? ",
    w("té", "tea"),
    ". —¿",
    w("Cómo", "how"),
    "? ",
    w("biblioteca", "library"),
    "(",
    w("s", "s"),
    ") “",
    w("lectura", "reading"),
    "”, (",
    w("a", "a"),
    "/",
    w("b", "b"),
    "), ",
    w("tarde", "afternoon"),
    ":",
  ],
};

function join(parts: Input[]): Input {
  return {
    raw: parts.map((part) => part.raw).join(" "),
    translated: parts.flatMap((part, i) =>
      i ? [" ", ...part.translated] : part.translated,
    ),
  };
}

const repeat = (count: number, input: Input) => join(Array(count).fill(input));
const dashWords = (gap: string) => [
  w("día", "day"),
  gap,
  w("noche", "night"),
  " ",
  w("tarde", "afternoon"),
  gap,
  w("mañana", "morning"),
];
export const pageFlipCases = [
  { name: "mixed punctuation", input: mixed },
  {
    name: "em dash with no spaces",
    input: repeat(17, {
      raw: "día—noche tarde—mañana",
      translated: dashWords("—"),
    }),
  },
  {
    name: "em dash with spaces on both sides",
    input: repeat(17, {
      raw: "día — noche tarde — mañana",
      translated: dashWords(" — "),
    }),
  },
  {
    name: "em dash with space before",
    input: repeat(17, {
      raw: "día —noche tarde —mañana",
      translated: dashWords(" —"),
    }),
  },
  {
    name: "em dash with space after",
    input: repeat(17, {
      raw: "día— noche tarde— mañana",
      translated: dashWords("— "),
    }),
  },
  {
    name: "slashes",
    input: repeat(18, {
      raw: "día/noche lector/autora",
      translated: [
        w("día", "day"),
        "/",
        w("noche", "night"),
        " ",
        w("lector", "reader"),
        "/",
        w("autora", "author"),
      ],
    }),
  },
  {
    name: "dotted initials",
    input: repeat(100, {
      raw: "A.V.",
      translated: [w("A", "A"), ".", w("V", "V"), "."],
    }),
  },
  {
    name: "curly apostrophes",
    input: join([
      repeat(18, {
        raw: "d’accord entra l’homme",
        translated: [
          w("d", "of"),
          "’",
          w("accord", "agreement"),
          " ",
          w("entra", "enters"),
          " ",
          w("l", "the"),
          "’",
          w("homme", "man"),
        ],
      }),
      {
        raw: "entra por allí",
        translated: [
          w("entra", "enters"),
          " ",
          w("por", "through"),
          " ",
          w("allí", "there"),
        ],
      },
    ]),
  },
  {
    name: "hyphenated words",
    input: join([
      repeat(10, {
        raw: "sur-oeste una entra luz lentamente sur-oeste",
        translated: [
          w("sur", "south"),
          "-",
          w("oeste", "west"),
          " ",
          w("una", "a"),
          " ",
          w("entra", "enters"),
          " ",
          w("luz", "light"),
          " ",
          w("lentamente", "slowly"),
          " ",
          w("sur", "south"),
          "-",
          w("oeste", "west"),
        ],
      }),
      { raw: "a", translated: [w("a", "to")] },
    ]),
  },
  {
    name: "three periods between words",
    input: join([
      repeat(30, {
        raw: "hoy...mañana",
        translated: [w("hoy", "today"), "...", w("mañana", "tomorrow")],
      }),
      { raw: "una luz", translated: [w("una", "a"), " ", w("luz", "light")] },
    ]),
  },
];

function mockParagraph({ raw, translated }: Input): SeedParagraph {
  const segments: ParagraphSegment[] = [];
  let word = 0;
  for (const token of translated) {
    if (typeof token === "string") {
      const last = segments.at(-1);
      if (last?.kind === "gap") last.text += token;
      else segments.push({ kind: "gap", text: token });
    } else {
      segments.push({
        kind: "word",
        ...token,
        sentence: 0,
        word,
        flatIndex: word,
        familiarity: 0,
      });
      word++;
    }
  }
  return { html: raw, segments };
}

export const lastParagraphId = 319;
export function pageFlipChapter(input: Input) {
  const last = lastParagraphId;
  return {
    paragraphs: Array.from({ length: last + 1 }, (_, p) => {
      if (p === last - 4) return mockParagraph(input);
      if (p === last - 6) return mockParagraph(repeat(8, mixed));
      if (p < 16) return mockParagraph(mixed);
      const sizes = [12, 70, 110, 35, 210, 100, 150];
      let count = sizes[p % sizes.length];
      if (p === last - 10 || p === last - 8) count = 540 + (p % 3) * 80;
      if (p === last - 1) count = 300;
      if (p === last) count = 1200;
      const indices = Array.from(
        { length: count },
        (_, i) => (i + p * 7) % proseSource.length,
      );
      return mockParagraph({
        raw: indices.map((i) => proseSource[i]).join(" "),
        translated: indices.flatMap((i, n): Token[] =>
          n ? [" ", proseTranslation[i]] : [proseTranslation[i]],
        ),
      });
    }),
  };
}
