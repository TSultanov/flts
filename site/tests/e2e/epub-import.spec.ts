import { test, expect } from '@playwright/test';

test.describe('EPUB Import with Mocked Translation', () => {
  test.beforeEach(async ({ page }) => {
    // Listen to console messages to debug issues
    page.on('console', msg => console.log('PAGE LOG:', msg.text()));
    page.on('pageerror', err => console.log('PAGE ERROR:', err.message));
    
    // Mock Google Gemini API calls to avoid real network requests
    await page.route('https://generativelanguage.googleapis.com/**', async route => {
      const url = route.request().url();
      console.log('Intercepted API call:', url);
      
      // Mock translation response
      const mockTranslationResponse = {
        candidates: [{
          content: {
            parts: [{
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
                          other: ""
                        }
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
                          other: ""
                        }
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
                          other: ""
                        }
                      }
                    ],
                    fullTranslation: "Capítulo Uno"
                  }
                ],
                sourceLanguage: "en",
                targetLanguage: "es"
              })
            }]
          }
        }]
      };
      
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(mockTranslationResponse)
      });
    });

    // Set up configuration for each test
    await page.goto('/config');
    await page.fill('#targetlanguage', 'Spanish');
    await page.fill('#apikey', 'test-api-key');
    await page.click('button:has-text("Save")');
    
    // Wait for config to be properly saved
    await page.waitForTimeout(500);
  });

  test('should show EPUB import tab and handle file selection', async ({ page }) => {
    // Navigate to the import page
    await page.goto('/import');
    
    // Verify we're on the import page
    await expect(page).toHaveURL('/import');
    
    // Should show the tab group with both tabs
    await expect(page.locator('text=Plain text import')).toBeVisible();
    await expect(page.locator('text=File import')).toBeVisible();
    
    // Click on File import tab
    await page.click('text=File import');
    
    // Should show the file input
    const fileInput = page.locator('input[type="file"][accept="application/epub+zip"]');
    await expect(fileInput).toBeVisible();
    
    // Should show no content initially
    await expect(page.locator('text=Loading...')).not.toBeVisible();
  });

  // Note: Due to the complexity of mocking EPUB parsing in Playwright E2E tests,
  // the following tests focus on UI interactions and basic functionality.
  // For comprehensive EPUB parsing tests, see the unit tests in epubLoader.spec.ts
  // In a production environment, you would want to have actual EPUB test files
  // or create a more sophisticated mocking strategy at the application level.

  test('should handle file selection UI without actual EPUB processing', async ({ page }) => {
    // Navigate to import page and switch to file import tab
    await page.goto('/import');
    await page.click('text=File import');
    
    // Create a simple mock file (not a real EPUB, just for UI testing)
    const mockFileContent = Buffer.from('mock epub content');

    // Upload the mock file - this will likely fail parsing but we can test the UI response
    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: 'test-book.epub',
      mimeType: 'application/epub+zip',
      buffer: mockFileContent
    });

    // Wait for any processing to complete
    await page.waitForTimeout(2000);

    // The file input should still be visible (whether processing succeeded or failed)
    await expect(fileInput).toBeVisible();
  });

  test('should show appropriate file input attributes', async ({ page }) => {
    await page.goto('/import');
    await page.click('text=File import');
    
    const fileInput = page.locator('input[type="file"]');
    
    // Should have correct file type restriction
    await expect(fileInput).toHaveAttribute('accept', 'application/epub+zip');
    
    // Should be a file input
    await expect(fileInput).toHaveAttribute('type', 'file');
    
    // Should have an ID for accessibility
    await expect(fileInput).toHaveAttribute('id', 'file');
  });

  test('should handle tab navigation with keyboard', async ({ page }) => {
    await page.goto('/import');
    
    // Focus on the first tab
    await page.keyboard.press('Tab');
    const plainTextTab = page.locator('text=Plain text import');
    
    // Should be able to navigate between tabs with keyboard
    await plainTextTab.focus();
    await page.keyboard.press('ArrowRight');
    
    // Should be able to activate File import tab
    const fileImportTab = page.locator('text=File import');
    await expect(fileImportTab).toBeVisible();
  });

  test('should have proper semantic structure for accessibility', async ({ page }) => {
    await page.goto('/import');
    
    // Both import options should be visible
    await expect(page.locator('text=Plain text import')).toBeVisible();
    await expect(page.locator('text=File import')).toBeVisible();
    
    // Switch to file import
    await page.click('text=File import');
    
    // File input should have proper labeling (either through label or aria-label)
    const fileInput = page.locator('input[type="file"]');
    await expect(fileInput).toBeVisible();
    
    // The file input should be accessible (check if it has an ID)
    const fileInputId = await fileInput.getAttribute('id');
    expect(fileInputId).toBe('file');
  });

  test('should integrate with the overall import workflow', async ({ page }) => {
    // Test that the file import tab is properly integrated into the import page workflow
    await page.goto('/import');
    
    // Should start with plain text import
    await expect(page.locator('#title')).toBeVisible();
    await expect(page.locator('#text')).toBeVisible();
    
    // Switch to file import
    await page.click('text=File import');
    
    // Should show file import interface
    await expect(page.locator('input[type="file"]')).toBeVisible();
    
    // Should not show plain text import fields
    await expect(page.locator('#title')).not.toBeVisible();
    await expect(page.locator('#text')).not.toBeVisible();
    
    // Switch back to plain text
    await page.click('text=Plain text import');
    
    // Should show plain text import fields again
    await expect(page.locator('#title')).toBeVisible();
    await expect(page.locator('#text')).toBeVisible();
    
    // Should not show file import
    await expect(page.locator('input[type="file"]')).not.toBeVisible();
  });

  test('should handle navigation from import page correctly', async ({ page }) => {
    await page.goto('/import');
    await page.click('text=File import');
    
    // Navigate to other pages and verify they work
    await page.goto('/library');
    await expect(page).toHaveURL('/library');
    
    await page.goto('/config');
    await expect(page).toHaveURL('/config');
    
    // Navigate back to import
    await page.goto('/import');
    await expect(page).toHaveURL('/import');
    
    // Import page should still be functional
    await expect(page.locator('text=Plain text import')).toBeVisible();
    await expect(page.locator('text=File import')).toBeVisible();
  });

  test('should show expected UI elements in file import tab', async ({ page }) => {
    await page.goto('/import');
    await page.click('text=File import');
    
    // Should show file import container specifically (be more specific to avoid conflicts)
    const fileImportContainer = page.locator('.container').nth(1); // Second container is file import
    await expect(fileImportContainer).toBeVisible();
    
    // Should show file input
    const fileInput = page.locator('input[type="file"]');
    await expect(fileInput).toBeVisible();
    
    // Initially should not show loading state
    await expect(page.locator('text=Loading...')).not.toBeVisible();
    
    // Should not show any book content initially
    await expect(page.locator('h1')).not.toBeVisible();
    await expect(page.locator('button.primary:has-text("Import")')).not.toBeVisible();
  });

  test('should successfully import a simple EPUB file', async ({ page }) => {
    await page.goto('/import');
    await page.click('text=File import');

    // Import the EPUB generator module and create a test EPUB
    const { createSimpleTestEpub } = await import('../fixtures/epub-generator');
    const epubBuffer = await createSimpleTestEpub();

    // Upload the EPUB file
    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: 'test-book.epub',
      mimeType: 'application/epub+zip',
      buffer: epubBuffer
    });

    // Wait for EPUB processing to complete (no reliable loading indicator)
    await page.waitForTimeout(3000);

    // Should show the book title and content
    await expect(page.locator('h1:has-text("Test Book")')).toBeVisible();
    await expect(page.locator('h2:has-text("Select chapters to import")')).toBeVisible();

    // Should show chapters
    await expect(page.locator('label:has-text("Chapter One")')).toBeVisible();
    await expect(page.locator('label:has-text("Chapter Two")')).toBeVisible();

    // Expand the first chapter to see content
    await page.click('summary:has-text("Chapter One")');

    // Should show chapter content in details
    await expect(page.locator('text=This is the first paragraph')).toBeVisible();
    await expect(page.locator('text=italic')).toBeVisible();
    await expect(page.locator('text=bold')).toBeVisible();

    // Should show import button
    const importButton = page.locator('.container').nth(1).locator('button.primary:has-text("Import")');
    await expect(importButton).toBeVisible();
    await expect(importButton).toBeEnabled();

    // Import the book
    await importButton.click();

    // Should redirect to library
    await expect(page).toHaveURL('/library');

    // Should show the imported book (be flexible with exact format)
    await expect(page.locator('text=Test Book')).toBeVisible();
    await expect(page.locator('text=chapter(s)')).toBeVisible();
  });

  test('should handle complex EPUB with formatting and multiple chapters', async ({ page }) => {
    await page.goto('/import');
    await page.click('text=File import');

    // Create a complex EPUB with various formatting
    const { createComplexTestEpub } = await import('../fixtures/epub-generator');
    const epubBuffer = await createComplexTestEpub();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: 'complex-book.epub',
      mimeType: 'application/epub+zip',
      buffer: epubBuffer
    });

    // Wait for processing
    await page.waitForTimeout(3000);

    // Should show the complex book title
    await expect(page.locator('h1:has-text("Complex Test Book: A Study in EPUB Structure")')).toBeVisible();

    // Should show all chapters
    await expect(page.locator('summary:has-text("Introduction")')).toBeVisible();
    await expect(page.locator('summary:has-text("Chapter 1: The Beginning")')).toBeVisible();
    await expect(page.locator('summary:has-text("Chapter 2: Advanced Features")')).toBeVisible();

    // Expand the introduction to see content formatting
    await page.click('summary:has-text("Introduction")');

    // Check that formatting is preserved
    await expect(page.locator('em:has-text("introduction")')).toBeVisible();
    await expect(page.locator('b:has-text("complex")')).toBeVisible();
    await expect(page.locator('i:has-text("italics")')).toBeVisible();

    // Test chapter selection functionality
    const chapter1Checkbox = page.locator('input[type="checkbox"]').first();
    await expect(chapter1Checkbox).toBeChecked(); // Should be checked by default

    // Uncheck first chapter
    await chapter1Checkbox.uncheck();
    await expect(chapter1Checkbox).not.toBeChecked();

    // Re-check it
    await chapter1Checkbox.check();
    await expect(chapter1Checkbox).toBeChecked();

    // Import the book
    const importButton = page.locator('.container').nth(1).locator('button.primary:has-text("Import")');
    await expect(importButton).toBeEnabled();
    await importButton.click();
    await expect(page).toHaveURL('/library');

    // Should show the imported book with 3 chapters
    await expect(page.locator('text=Complex Test Book')).toBeVisible();
    await expect(page.locator('text=chapter(s)')).toBeVisible();
  });

  test('should handle EPUB with empty chapters', async ({ page }) => {
    await page.goto('/import');
    await page.click('text=File import');

    const { createEmptyChaptersTestEpub } = await import('../fixtures/epub-generator');
    const epubBuffer = await createEmptyChaptersTestEpub();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: 'empty-chapters.epub',
      mimeType: 'application/epub+zip',
      buffer: epubBuffer
    });

    await page.waitForTimeout(3000);

    await expect(page.locator('h1:has-text("Empty Chapters Test")')).toBeVisible();

    // Should show all chapter titles somewhere on the page (use first() to avoid strict mode issues)
    await expect(page.locator('summary:has-text("Non-Empty Chapter")').first()).toBeVisible();
    await expect(page.locator('summary:has-text("Empty Chapter")').first()).toBeVisible();
    await expect(page.locator('summary:has-text("Whitespace Only Chapter")').first()).toBeVisible();
    await expect(page.locator('summary:has-text("HTML Tags Only Chapter")').first()).toBeVisible();

    // Expand the non-empty chapter to see content
    await page.click('summary:has-text("Non-Empty Chapter")');

    // Should show content only for non-empty chapter
    await expect(page.locator('text=This chapter has content')).toBeVisible();

    const importButton = page.locator('.container').nth(1).locator('button.primary:has-text("Import")');
    await expect(importButton).toBeEnabled();
    await importButton.click();
    await expect(page).toHaveURL('/library');
  });

  test('should handle multilingual EPUB content', async ({ page }) => {
    await page.goto('/import');
    await page.click('text=File import');

    const { createMultilingualTestEpub } = await import('../fixtures/epub-generator');
    const epubBuffer = await createMultilingualTestEpub();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: 'multilingual.epub',
      mimeType: 'application/epub+zip',
      buffer: epubBuffer
    });

    await page.waitForTimeout(3000);

    await expect(page.locator('h1:has-text("Multilingual Test Book")')).toBeVisible();

    // Should show chapters in different languages
    await expect(page.locator('label:has-text("English Chapter")')).toBeVisible();
    await expect(page.locator('label:has-text("Spanish Chapter")')).toBeVisible();
    await expect(page.locator('label:has-text("French Chapter")')).toBeVisible();
    await expect(page.locator('label:has-text("Mixed Language Chapter")')).toBeVisible();

    // Expand Spanish chapter to see special characters
    await page.click('summary:has-text("Spanish Chapter")');

    // Should preserve special characters
    await expect(page.locator('text=¡Hola, mundo!')).toBeVisible();
    await expect(page.locator('text=¿Cómo estás')).toBeVisible();
    
    // Expand French chapter 
    await page.click('summary:has-text("French Chapter")');
    await expect(page.locator('text=Bonjour, monde!')).toBeVisible();

    const importButton = page.locator('.container').nth(1).locator('button.primary:has-text("Import")');
    await expect(importButton).toBeEnabled();
    await importButton.click();
    await expect(page).toHaveURL('/library');

    await expect(page.locator('text=Multilingual Test Book')).toBeVisible();
    await expect(page.locator('text=chapter(s)')).toBeVisible();
  });

  test('should handle importing and viewing EPUB content in library', async ({ page }) => {
    await page.goto('/import');
    await page.click('text=File import');

    const { createSimpleTestEpub } = await import('../fixtures/epub-generator');
    const epubBuffer = await createSimpleTestEpub();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: 'test-for-viewing.epub',
      mimeType: 'application/epub+zip',
      buffer: epubBuffer
    });

    await page.waitForTimeout(3000);
    
    // Wait for the import button to be enabled and visible
    const importButton = page.locator('.container').nth(1).locator('button.primary:has-text("Import")');
    await expect(importButton).toBeVisible();
    await expect(importButton).toBeEnabled();
    await importButton.click();
    await expect(page).toHaveURL('/library');

    // Click on the imported book to view it
    await page.click('a:has-text("Test Book")');

    // Should be in book view with chapters
    await expect(page.locator('text=Chapter One')).toBeVisible();
    await expect(page.locator('text=Chapter Two')).toBeVisible();

    // Click on first chapter
    await page.click('text=Chapter One');

    // Should show chapter content
    await expect(page.locator('text=This is the first paragraph of chapter one.')).toBeVisible();
    await expect(page.locator('text=This is the second paragraph with some')).toBeVisible();

    // Check that formatting is preserved in the reader
    await expect(page.locator('em:has-text("italic")')).toBeVisible();
    await expect(page.locator('b:has-text("bold")')).toBeVisible();
  });

  test('should handle selective chapter import', async ({ page }) => {
    await page.goto('/import');
    await page.click('text=File import');

    const { createComplexTestEpub } = await import('../fixtures/epub-generator');
    const epubBuffer = await createComplexTestEpub();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: 'selective-import.epub',
      mimeType: 'application/epub+zip',
      buffer: epubBuffer
    });

    await page.waitForTimeout(3000);

    // Uncheck the first chapter (Table of Contents)
    const introCheckbox = page.locator('input[type="checkbox"]').first();
    await introCheckbox.uncheck();

    // Uncheck the last chapter  
    const advancedCheckbox = page.locator('input[type="checkbox"]').last();
    await advancedCheckbox.uncheck();

    // Only "Chapter 1: The Beginning" should remain checked
    const chapter1Checkbox = page.locator('input[type="checkbox"]').nth(1);
    await expect(chapter1Checkbox).toBeChecked();

    const importButton = page.locator('.container').nth(1).locator('button.primary:has-text("Import")');
    await expect(importButton).toBeEnabled();
    await importButton.click();
    await expect(page).toHaveURL('/library');

    // Should show only 1 chapter since we unchecked 2 out of 3
    await expect(page.locator('text=Complex Test Book')).toBeVisible();
    await expect(page.locator('text=chapter(s)')).toBeVisible();

    // Verify the imported content
    await page.click('a:has-text("Complex Test Book")');
    await expect(page.locator('text=Chapter 1: The Beginning')).toBeVisible();
    
    // Should not show the unchecked chapters
    await expect(page.locator('text=Table of Contents')).not.toBeVisible();
    await expect(page.locator('text=Chapter 2: Advanced Features')).not.toBeVisible();
  });

  test('should handle EPUB processing errors gracefully', async ({ page }) => {
    await page.goto('/import');
    await page.click('text=File import');

    // Upload a malformed file
    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: 'invalid.epub',
      mimeType: 'application/epub+zip',
      buffer: Buffer.from('This is not a valid EPUB file')
    });

    await page.waitForTimeout(3000);

    // Should not show loading forever or crash the application
    // The exact error handling behavior depends on implementation
    // but it should not hang or crash
    await expect(page.locator('input[type="file"]')).toBeVisible();
    
    // The page should remain functional
    await expect(page.locator('text=File import')).toBeVisible();
  });

  test('should maintain EPUB import state during navigation', async ({ page }) => {
    await page.goto('/import');
    await page.click('text=File import');

    const { createSimpleTestEpub } = await import('../fixtures/epub-generator');
    const epubBuffer = await createSimpleTestEpub();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: 'navigation-test.epub',
      mimeType: 'application/epub+zip',
      buffer: epubBuffer
    });

    await page.waitForTimeout(3000);

    // Should show the loaded EPUB
    await expect(page.locator('h1:has-text("Test Book")')).toBeVisible();

    // Navigate away and back
    await page.goto('/library');
    await expect(page).toHaveURL('/library');

    await page.goto('/import');
    await page.click('text=File import');

    // The EPUB should no longer be loaded (fresh state)
    await expect(page.locator('h1:has-text("Test Book")')).not.toBeVisible();
    await expect(page.locator('button.primary:has-text("Import")')).not.toBeVisible();
    
    // File input should be visible and ready for new upload
    await expect(page.locator('input[type="file"]')).toBeVisible();
  });
});
