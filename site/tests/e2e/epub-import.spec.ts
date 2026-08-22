import { test, expect } from "./helpers/test";
import { isRealMode } from "./helpers/backend-mode";

async function openFileImport(page: import("@playwright/test").Page) {
  await page.goto("/import");
  await page.click("text=File import");
}

async function uploadEpub(
  page: import("@playwright/test").Page,
  buffer: Buffer,
  name: string,
) {
  await page.locator('input[type="file"]').setInputFiles({
    name,
    mimeType: "application/epub+zip",
    buffer,
  });
  await page.waitForSelector("h1", { timeout: 10000 });
}

// Plain-text import also has #src-lang; both tabs stay in the DOM.
function srcLang(page: import("@playwright/test").Page) {
  return page.locator(".container").nth(1).locator("#src-lang");
}

test.describe("EPUB Import with Mocked Translation", () => {
  test.beforeEach(async ({ page }) => {
    page.on("console", (msg) => console.log("PAGE LOG:", msg.text()));
    page.on("pageerror", (err) => console.log("PAGE ERROR:", err.message));

    await page.route(
      "https://generativelanguage.googleapis.com/**",
      async (route) => {
        const url = route.request().url();
        console.log("Intercepted API call:", url);

        const mockTranslationResponse = {
          candidates: [
            {
              content: {
                parts: [
                  {
                    text: JSON.stringify({
                      sentences: [
                        {
                          words: [
                            {
                              original: "Chapter",
                              isPunctuation: false,
                              isStandalonePunctuation: false,
                              isOpeningParenthesis: false,
                              isClosingParenthesis: false,
                              translations: ["Capítulo"],
                              note: "A section of a book",
                              grammar: {
                                originalInitialForm: "chapter",
                                targetInitialForm: "capítulo",
                                partOfSpeech: "noun",
                                plurality: "singular",
                                person: "",
                                tense: "",
                                case: "nominative",
                                other: "",
                              },
                            },
                            {
                              original: " ",
                              isPunctuation: true,
                              isStandalonePunctuation: false,
                              isOpeningParenthesis: false,
                              isClosingParenthesis: false,
                              translations: [" "],
                              note: "",
                              grammar: {
                                originalInitialForm: " ",
                                targetInitialForm: " ",
                                partOfSpeech: "space",
                                plurality: "",
                                person: "",
                                tense: "",
                                case: "",
                                other: "",
                              },
                            },
                            {
                              original: "One",
                              isPunctuation: false,
                              isStandalonePunctuation: false,
                              isOpeningParenthesis: false,
                              isClosingParenthesis: false,
                              translations: ["Uno"],
                              note: "The number 1",
                              grammar: {
                                originalInitialForm: "one",
                                targetInitialForm: "uno",
                                partOfSpeech: "number",
                                plurality: "singular",
                                person: "",
                                tense: "",
                                case: "",
                                other: "",
                              },
                            },
                          ],
                          fullTranslation: "Capítulo Uno",
                        },
                      ],
                      sourceLanguage: "en",
                      targetLanguage: "es",
                    }),
                  },
                ],
              },
            },
          ],
        };

        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(mockTranslationResponse),
        });
      },
    );

    await page.goto("/");
    await page.waitForURL("/library");
  });

  test("should show EPUB import tab and handle file selection", async ({
    page,
  }) => {
    await page.goto("/import");

    await expect(page).toHaveURL("/import");

    await expect(page.locator("text=Plain text import")).toBeVisible();
    await expect(page.locator("text=File import")).toBeVisible();

    await page.click("text=File import");

    const fileInput = page.locator(
      'input[type="file"][accept="application/epub+zip"]',
    );
    await expect(fileInput).toBeVisible();

    await expect(page.locator("text=Loading...")).not.toBeVisible();
  });

  test("should handle file selection UI without actual EPUB processing", async ({
    page,
  }) => {
    await page.goto("/import");
    await page.click("text=File import");

    const mockFileContent = Buffer.from("mock epub content");

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: "test-book.epub",
      mimeType: "application/epub+zip",
      buffer: mockFileContent,
    });

    await page.waitForTimeout(2000);

    await expect(fileInput).toBeVisible();
  });

  test("should show appropriate file input attributes", async ({ page }) => {
    await page.goto("/import");
    await page.click("text=File import");

    const fileInput = page.locator('input[type="file"]');

    await expect(fileInput).toHaveAttribute("accept", "application/epub+zip");

    await expect(fileInput).toHaveAttribute("type", "file");

    await expect(fileInput).toHaveAttribute("id", "file");
  });

  test("should handle tab navigation with keyboard", async ({ page }) => {
    await page.goto("/import");

    await page.keyboard.press("Tab");
    const plainTextTab = page.locator("text=Plain text import");

    await plainTextTab.focus();
    await page.keyboard.press("ArrowRight");

    const fileImportTab = page.locator("text=File import");
    await expect(fileImportTab).toBeVisible();
  });

  test("should have proper semantic structure for accessibility", async ({
    page,
  }) => {
    await page.goto("/import");

    await expect(page.locator("text=Plain text import")).toBeVisible();
    await expect(page.locator("text=File import")).toBeVisible();

    await page.click("text=File import");

    const fileInput = page.locator('input[type="file"]');
    await expect(fileInput).toBeVisible();

    const fileInputId = await fileInput.getAttribute("id");
    expect(fileInputId).toBe("file");
  });

  test("should integrate with the overall import workflow", async ({
    page,
  }) => {
    await page.goto("/import");

    await expect(page.locator("#title")).toBeVisible();
    await expect(page.locator("#text")).toBeVisible();

    await page.click("text=File import");

    await expect(page.locator('input[type="file"]')).toBeVisible();

    await expect(page.locator("#title")).not.toBeVisible();
    await expect(page.locator("#text")).not.toBeVisible();

    await page.click("text=Plain text import");

    await expect(page.locator("#title")).toBeVisible();
    await expect(page.locator("#text")).toBeVisible();

    await expect(page.locator('input[type="file"]')).not.toBeVisible();
  });

  test("should handle navigation from import page correctly", async ({
    page,
  }) => {
    await page.goto("/import");
    await page.click("text=File import");

    await page.goto("/library");
    await expect(page).toHaveURL("/library");

    await page.goto("/config");
    await expect(page).toHaveURL("/config");

    await page.goto("/import");
    await expect(page).toHaveURL("/import");

    await expect(page.locator("text=Plain text import")).toBeVisible();
    await expect(page.locator("text=File import")).toBeVisible();
  });

  test("should show expected UI elements in file import tab", async ({
    page,
  }) => {
    await page.goto("/import");
    await page.click("text=File import");

    const fileImportContainer = page.locator(".container").nth(1); // Second container is file import
    await expect(fileImportContainer).toBeVisible();

    const fileInput = page.locator('input[type="file"]');
    await expect(fileInput).toBeVisible();

    await expect(page.locator("text=Loading...")).not.toBeVisible();

    await expect(page.locator("h1")).not.toBeVisible();
    await expect(
      page.locator('button.primary:has-text("Import")'),
    ).not.toBeVisible();
  });

  test("should successfully import a simple EPUB file", async ({ page }) => {
    await page.goto("/import");
    await page.click("text=File import");

    const { createSimpleTestEpub } = await import("../fixtures/epub-generator");
    const epubBuffer = await createSimpleTestEpub();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: "test-book.epub",
      mimeType: "application/epub+zip",
      buffer: epubBuffer,
    });

    await page.waitForSelector("h1", { timeout: 10000 });

    await expect(page.locator('h1:has-text("Test Book")')).toBeVisible();
    await expect(
      page.locator('h2:has-text("Select chapters to import")'),
    ).toBeVisible();

    await expect(page.locator('label:has-text("Chapter One")')).toBeVisible();
    await expect(page.locator('label:has-text("Chapter Two")')).toBeVisible();

    await page.click('summary:has-text("Chapter One")');

    await expect(
      page.locator("text=This is the first paragraph"),
    ).toBeVisible();
    await expect(page.locator("text=italic")).toBeVisible();
    await expect(page.locator("text=bold")).toBeVisible();

    const importButton = page
      .locator(".container")
      .nth(1)
      .locator('button.primary:has-text("Import")');
    await expect(importButton).toBeVisible();
    await expect(importButton).toBeEnabled();

    await importButton.click();

    await expect(page).toHaveURL("/library");

    await expect(page.locator("text=Test Book")).toBeVisible();
    await expect(page.locator("text=chapter(s)")).toBeVisible();
  });

  test("should handle complex EPUB with formatting and multiple chapters", async ({
    page,
  }) => {
    await page.goto("/import");
    await page.click("text=File import");

    const { createComplexTestEpub } = await import(
      "../fixtures/epub-generator"
    );
    const epubBuffer = await createComplexTestEpub();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: "complex-book.epub",
      mimeType: "application/epub+zip",
      buffer: epubBuffer,
    });

    await page.waitForSelector("h1", { timeout: 15000 });

    await expect(
      page.locator(
        'h1:has-text("Complex Test Book: A Study in EPUB Structure")',
      ),
    ).toBeVisible();

    // Firefox needs extra time for complex content.
    await page.waitForSelector("summary", { timeout: 10000 });

    await expect(
      page.locator('summary:has-text("Introduction")'),
    ).toBeVisible();
    await expect(
      page.locator('summary:has-text("Chapter 1: The Beginning")'),
    ).toBeVisible();
    await expect(
      page.locator('summary:has-text("Chapter 2: Advanced Features")'),
    ).toBeVisible();

    await page.click('summary:has-text("Introduction")');

    await expect(page.locator('em:has-text("introduction")')).toBeVisible();
    await expect(page.locator('b:has-text("complex")')).toBeVisible();
    await expect(page.locator('i:has-text("italics")')).toBeVisible();

    const chapter1Checkbox = page.locator('input[type="checkbox"]').first();
    await expect(chapter1Checkbox).toBeChecked(); // Should be checked by default

    await chapter1Checkbox.uncheck();
    await expect(chapter1Checkbox).not.toBeChecked();

    await chapter1Checkbox.check();
    await expect(chapter1Checkbox).toBeChecked();

    const importButton = page
      .locator(".container")
      .nth(1)
      .locator('button.primary:has-text("Import")');
    await expect(importButton).toBeEnabled();
    await importButton.click();
    await expect(page).toHaveURL("/library");

    await expect(page.locator("text=Complex Test Book")).toBeVisible();
    await expect(page.locator("text=chapter(s)")).toBeVisible();
  });

  test("should handle EPUB with empty chapters", async ({ page }) => {
    await page.goto("/import");
    await page.click("text=File import");

    const { createEmptyChaptersTestEpub } = await import(
      "../fixtures/epub-generator"
    );
    const epubBuffer = await createEmptyChaptersTestEpub();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: "empty-chapters.epub",
      mimeType: "application/epub+zip",
      buffer: epubBuffer,
    });

    await page.waitForSelector("h1", { timeout: 10000 });

    await expect(
      page.locator('h1:has-text("Empty Chapters Test")'),
    ).toBeVisible();

    await expect(
      page.locator('summary:has-text("Non-Empty Chapter")').first(),
    ).toBeVisible();
    await expect(
      page.locator('summary:has-text("Empty Chapter")').first(),
    ).toBeVisible();
    await expect(
      page.locator('summary:has-text("Whitespace Only Chapter")').first(),
    ).toBeVisible();
    await expect(
      page.locator('summary:has-text("HTML Tags Only Chapter")').first(),
    ).toBeVisible();

    await page.click('summary:has-text("Non-Empty Chapter")');

    await expect(page.locator("text=This chapter has content")).toBeVisible();

    const importButton = page
      .locator(".container")
      .nth(1)
      .locator('button.primary:has-text("Import")');
    await expect(importButton).toBeEnabled();
    await importButton.click();
    await expect(page).toHaveURL("/library");
  });

  test("should handle multilingual EPUB content", async ({ page }) => {
    await page.goto("/import");
    await page.click("text=File import");

    const { createMultilingualTestEpub } = await import(
      "../fixtures/epub-generator"
    );
    const epubBuffer = await createMultilingualTestEpub();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: "multilingual.epub",
      mimeType: "application/epub+zip",
      buffer: epubBuffer,
    });

    await page.waitForSelector("h1", { timeout: 10000 });

    await expect(
      page.locator('h1:has-text("Multilingual Test Book")'),
    ).toBeVisible();

    await expect(
      page.locator('label:has-text("English Chapter")'),
    ).toBeVisible();
    await expect(
      page.locator('label:has-text("Spanish Chapter")'),
    ).toBeVisible();
    await expect(
      page.locator('label:has-text("French Chapter")'),
    ).toBeVisible();
    await expect(
      page.locator('label:has-text("Mixed Language Chapter")'),
    ).toBeVisible();

    await page.click('summary:has-text("Spanish Chapter")');

    await expect(page.locator("text=¡Hola, mundo!")).toBeVisible();
    await expect(page.locator("text=¿Cómo estás")).toBeVisible();

    await page.click('summary:has-text("French Chapter")');
    await expect(page.locator("text=Bonjour, monde!")).toBeVisible();

    const importButton = page
      .locator(".container")
      .nth(1)
      .locator('button.primary:has-text("Import")');
    await expect(importButton).toBeEnabled();
    await importButton.click();
    await expect(page).toHaveURL("/library");

    await expect(page.locator("text=Multilingual Test Book")).toBeVisible();
    await expect(page.locator("text=chapter(s)")).toBeVisible();
  });

  test("should handle importing and viewing EPUB content in library", async ({
    page,
  }) => {
    await page.goto("/import");
    await page.click("text=File import");

    const { createSimpleTestEpub } = await import("../fixtures/epub-generator");
    const epubBuffer = await createSimpleTestEpub();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: "test-for-viewing.epub",
      mimeType: "application/epub+zip",
      buffer: epubBuffer,
    });

    await page.waitForSelector("h1", { timeout: 10000 });

    const importButton = page
      .locator(".container")
      .nth(1)
      .locator('button.primary:has-text("Import")');
    await expect(importButton).toBeVisible();
    await expect(importButton).toBeEnabled();
    await importButton.click();
    await expect(page).toHaveURL("/library");

    await page.click('a:has-text("Test Book")');

    // Chapters live inside the collapsible ChaptersPanel; open it.
    await page.locator('[data-testid="chapters-panel-handle"]').click();

    await expect(page.locator("text=Chapter One")).toBeVisible();
    await expect(page.locator("text=Chapter Two")).toBeVisible();

    await page.click("text=Chapter One");

    await expect(
      page.locator("text=This is the first paragraph of chapter one."),
    ).toBeVisible();
    await expect(
      page.locator("text=This is the second paragraph with some"),
    ).toBeVisible();

    await expect(page.locator('em:has-text("italic")')).toBeVisible();
    await expect(page.locator('b:has-text("bold")')).toBeVisible();
  });

  test("should handle selective chapter import", async ({ page }) => {
    await page.goto("/import");
    await page.click("text=File import");

    const { createComplexTestEpub } = await import(
      "../fixtures/epub-generator"
    );
    const epubBuffer = await createComplexTestEpub();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: "selective-import.epub",
      mimeType: "application/epub+zip",
      buffer: epubBuffer,
    });

    await page.waitForSelector("h1", { timeout: 15000 });

    // Firefox needs extra time for complex content.
    await page.waitForSelector("summary", { timeout: 10000 });

    const introCheckbox = page.locator('input[type="checkbox"]').first();
    await introCheckbox.uncheck();

    const advancedCheckbox = page.locator('input[type="checkbox"]').last();
    await advancedCheckbox.uncheck();

    const chapter1Checkbox = page.locator('input[type="checkbox"]').nth(1);
    await expect(chapter1Checkbox).toBeChecked();

    const importButton = page
      .locator(".container")
      .nth(1)
      .locator('button.primary:has-text("Import")');
    await expect(importButton).toBeEnabled();
    await importButton.click();
    await expect(page).toHaveURL("/library");

    await expect(page.locator("text=Complex Test Book")).toBeVisible();
    await expect(page.locator("text=chapter(s)")).toBeVisible();

    await page.click('a:has-text("Complex Test Book")');
    await expect(page.locator("text=Chapter 1: The Beginning")).toBeVisible();

    await expect(page.locator("text=Table of Contents")).not.toBeVisible();
    await expect(
      page.locator("text=Chapter 2: Advanced Features"),
    ).not.toBeVisible();
  });

  test("should handle EPUB processing errors gracefully", async ({ page }) => {
    await page.goto("/import");
    await page.click("text=File import");

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: "invalid.epub",
      mimeType: "application/epub+zip",
      buffer: Buffer.from("This is not a valid EPUB file"),
    });

    await page.waitForTimeout(3000);

    await expect(page.locator('input[type="file"]')).toBeVisible();

    await expect(page.locator("text=File import")).toBeVisible();
  });

  test("should maintain EPUB import state during navigation", async ({
    page,
  }) => {
    await page.goto("/import");
    await page.click("text=File import");

    const { createSimpleTestEpub } = await import("../fixtures/epub-generator");
    const epubBuffer = await createSimpleTestEpub();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: "navigation-test.epub",
      mimeType: "application/epub+zip",
      buffer: epubBuffer,
    });

    await page.waitForSelector("h1", { timeout: 10000 });

    await expect(page.locator('h1:has-text("Test Book")')).toBeVisible();

    await page.goto("/library");
    await expect(page).toHaveURL("/library");

    await page.goto("/import");
    await page.click("text=File import");

    await expect(page.locator('h1:has-text("Test Book")')).not.toBeVisible();
    await expect(
      page.locator('button.primary:has-text("Import")'),
    ).not.toBeVisible();

    await expect(page.locator('input[type="file"]')).toBeVisible();
  });

  test("preselects Spanish from dc:language es", async ({ page }) => {
    await openFileImport(page);
    const { createTestEpub } = await import("../fixtures/epub-generator");
    const buffer = await createTestEpub({
      title: "Libro",
      chapters: [{ title: "Uno", content: "<p>Hola.</p>" }],
      language: "es",
    });
    await uploadEpub(page, buffer, "es.epub");
    await expect(srcLang(page)).toHaveValue("spa");
  });

  test("preselects German from BCP-47 de-DE", async ({ page }) => {
    await openFileImport(page);
    const { createTestEpub } = await import("../fixtures/epub-generator");
    const buffer = await createTestEpub({
      title: "Buch",
      chapters: [{ title: "Eins", content: "<p>Hallo.</p>" }],
      language: "de-DE",
    });
    await uploadEpub(page, buffer, "de.epub");
    await expect(srcLang(page)).toHaveValue("deu");
  });

  test("keeps eng when dc:language is missing", async ({ page }) => {
    await openFileImport(page);
    const { createTestEpub } = await import("../fixtures/epub-generator");
    const buffer = await createTestEpub({
      title: "No Lang",
      chapters: [{ title: "Ch", content: "<p>Hi.</p>" }],
      language: null,
    });
    await uploadEpub(page, buffer, "none.epub");
    await expect(srcLang(page)).toHaveValue("eng");
  });

  test("keeps eng when dc:language is unparseable", async ({ page }) => {
    await openFileImport(page);
    const { createTestEpub } = await import("../fixtures/epub-generator");
    const buffer = await createTestEpub({
      title: "Bad Lang",
      chapters: [{ title: "Ch", content: "<p>Hi.</p>" }],
      // isolang treats primary subtag `not` as ISO 639-3; ??? is unparseable.
      language: "???",
    });
    await uploadEpub(page, buffer, "bad.epub");
    await expect(srcLang(page)).toHaveValue("eng");
  });

  test("keeps eng when parsed language is not in the dropdown", async ({
    page,
  }) => {
    // Premise is the mock's short language list; the real backend does offer nld.
    test.skip(isRealMode(), "depends on the mock language list");
    // mock parse_language_id maps nl → nld; nld is not in mockLanguages
    await openFileImport(page);
    const { createTestEpub } = await import("../fixtures/epub-generator");
    const buffer = await createTestEpub({
      title: "Boek",
      chapters: [{ title: "Een", content: "<p>Hallo.</p>" }],
      language: "nl",
    });
    await uploadEpub(page, buffer, "nl.epub");
    await expect(srcLang(page)).toHaveValue("eng");
  });

  test("lets the user override a preselected language", async ({ page }) => {
    await openFileImport(page);
    const { createTestEpub } = await import("../fixtures/epub-generator");
    const buffer = await createTestEpub({
      title: "Libro",
      chapters: [{ title: "Uno", content: "<p>Hola.</p>" }],
      language: "es",
    });
    await uploadEpub(page, buffer, "es.epub");
    await expect(srcLang(page)).toHaveValue("spa");
    await srcLang(page).selectOption("deu");
    await expect(srcLang(page)).toHaveValue("deu");
  });

  test("resets to eng when a later file has no language", async ({ page }) => {
    await openFileImport(page);
    const { createTestEpub } = await import("../fixtures/epub-generator");
    const spanish = await createTestEpub({
      title: "Libro",
      chapters: [{ title: "Uno", content: "<p>Hola.</p>" }],
      language: "es",
    });
    await uploadEpub(page, spanish, "es.epub");
    await expect(srcLang(page)).toHaveValue("spa");

    const none = await createTestEpub({
      title: "No Lang",
      chapters: [{ title: "Ch", content: "<p>Hi.</p>" }],
      language: null,
    });
    await uploadEpub(page, none, "none.epub");
    await expect(srcLang(page)).toHaveValue("eng");
  });
});
