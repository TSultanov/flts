import { expect, test, type Page } from '@playwright/test';

// Public-behavior tests for the three library dialogs. Drives the real entry
// points (Select All -> Move/Delete Selected) so the tests survive any
// internal refactor of the dialog plumbing.

async function seedBook(page: Page, title: string): Promise<void> {
  await page.evaluate((t) => {
    (window as any).__test.seedBook({
      title: t,
      chapters: [{ paragraphs: [{ html: '<p>x</p>' }] }],
    });
  }, title);
}

async function setupLibraryWithTwoBooks(page: Page) {
  await page.goto('/library');
  await seedBook(page, 'Book A');
  await seedBook(page, 'Book B');
  await expect(page.getByTestId('book-checkbox')).toHaveCount(2);
  await page.getByTestId('select-all-button').click();
  await expect(page.getByTestId('selection-count')).toHaveText('2 selected');
}

test.describe('Library dialogs', () => {
  test.beforeEach(async ({ page }) => {
    await setupLibraryWithTwoBooks(page);
  });

  // -------- ConfirmDialog (via batch-delete) --------

  test('ConfirmDialog: opens with title and count in message', async ({ page }) => {
    await page.getByTestId('delete-selected-button').click();
    const dialog = page.getByTestId('confirm-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('h3')).toHaveText('Delete Books');
    await expect(dialog.getByTestId('confirm-dialog-message')).toContainText('2 book(s)');
  });

  test('ConfirmDialog: Cancel closes dialog and leaves books', async ({ page }) => {
    await page.getByTestId('delete-selected-button').click();
    await page.getByTestId('confirm-dialog-cancel').click();
    await expect(page.getByTestId('confirm-dialog')).toBeHidden();
    await expect(page.getByTestId('book-checkbox')).toHaveCount(2);
  });

  test('ConfirmDialog: Confirm closes dialog and removes selected books', async ({ page }) => {
    await page.getByTestId('delete-selected-button').click();
    await page.getByTestId('confirm-dialog-confirm').click();
    await expect(page.getByTestId('confirm-dialog')).toBeHidden();
    await expect(page.getByTestId('book-checkbox')).toHaveCount(0);
  });

  test('ConfirmDialog: Escape closes dialog and leaves books', async ({ page }) => {
    await page.getByTestId('delete-selected-button').click();
    await expect(page.getByTestId('confirm-dialog')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('confirm-dialog')).toBeHidden();
    await expect(page.getByTestId('book-checkbox')).toHaveCount(2);
  });

  // -------- MoveFolderDialog (via batch-move) --------

  test('MoveFolderDialog: opens with heading and renders root folder', async ({ page }) => {
    await page.getByTestId('move-selected-button').click();
    const dialog = page.getByTestId('move-folder-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('h3')).toHaveText('Move to Folder');
    await expect(
      dialog.locator('[data-testid="folder-button"][data-folder-path=""]'),
    ).toBeVisible();
  });

  test('MoveFolderDialog: Cancel closes dialog and leaves books in place', async ({ page }) => {
    await page.getByTestId('move-selected-button').click();
    await page.getByTestId('move-folder-cancel').click();
    await expect(page.getByTestId('move-folder-dialog')).toBeHidden();
    await expect(page.getByTestId('book-checkbox')).toHaveCount(2);
  });

  test('MoveFolderDialog: Escape closes dialog and leaves books in place', async ({ page }) => {
    await page.getByTestId('move-selected-button').click();
    await expect(page.getByTestId('move-folder-dialog')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('move-folder-dialog')).toBeHidden();
    await expect(page.getByTestId('book-checkbox')).toHaveCount(2);
  });

  test('MoveFolderDialog: Create new folder opens nested CreateFolderDialog', async ({ page }) => {
    await page.getByTestId('move-selected-button').click();
    await page.getByTestId('create-new-folder-button').first().click();
    await expect(page.getByTestId('move-folder-dialog')).toBeVisible();
    await expect(page.getByTestId('create-folder-dialog')).toBeVisible();
  });

  test('MoveFolderDialog: Confirm with newly created folder moves books into it', async ({ page }) => {
    await page.getByTestId('move-selected-button').click();
    await page.getByTestId('create-new-folder-button').first().click();
    await page.getByTestId('create-folder-input').fill('Archive');
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('create-folder-dialog')).toBeHidden();
    await expect(page.getByTestId('move-to-preview')).toContainText('Move to: /Archive');

    await page.getByTestId('move-folder-confirm').click();
    await expect(page.getByTestId('move-folder-dialog')).toBeHidden();

    // After move, the library re-renders with books grouped under "Archive".
    await expect(page.locator('details > summary', { hasText: 'Archive' })).toBeVisible();
    await expect(page.getByTestId('book-checkbox')).toHaveCount(2);
  });

  // -------- CreateFolderDialog (nested inside MoveFolderDialog) --------

  async function openCreateFolderDialog(page: Page) {
    await page.getByTestId('move-selected-button').click();
    await page.getByTestId('create-new-folder-button').first().click();
    await expect(page.getByTestId('create-folder-dialog')).toBeVisible();
  }

  test('CreateFolderDialog: opens with heading and focuses input', async ({ page }) => {
    await openCreateFolderDialog(page);
    await expect(page.getByTestId('create-folder-dialog').locator('h3')).toHaveText(
      'Create New Folder',
    );
    await expect(page.getByTestId('create-folder-input')).toBeFocused();
  });

  test('CreateFolderDialog: submit is disabled empty, enabled with text', async ({ page }) => {
    await openCreateFolderDialog(page);
    const submit = page.getByTestId('create-folder-submit');
    await expect(submit).toBeDisabled();
    await page.getByTestId('create-folder-input').fill('x');
    await expect(submit).toBeEnabled();
  });

  test('CreateFolderDialog: Enter creates folder, closes, and auto-selects it', async ({ page }) => {
    await openCreateFolderDialog(page);
    await page.getByTestId('create-folder-input').fill('Mythology');
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('create-folder-dialog')).toBeHidden();
    await expect(
      page.locator('[data-testid="folder-button"][data-folder-path="Mythology"]'),
    ).toBeVisible();
    await expect(page.getByTestId('move-to-preview')).toContainText('Move to: /Mythology');
  });

  test('CreateFolderDialog: Escape closes without creating', async ({ page }) => {
    await openCreateFolderDialog(page);
    await page.getByTestId('create-folder-input').fill('NeverCreated');
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('create-folder-dialog')).toBeHidden();
    await expect(
      page.locator('[data-testid="folder-button"][data-folder-path="NeverCreated"]'),
    ).toHaveCount(0);
  });

  test('CreateFolderDialog: Cancel closes without creating', async ({ page }) => {
    await openCreateFolderDialog(page);
    await page.getByTestId('create-folder-input').fill('NeverCreated');
    await page.getByTestId('create-folder-cancel').click();
    await expect(page.getByTestId('create-folder-dialog')).toBeHidden();
    await expect(
      page.locator('[data-testid="folder-button"][data-folder-path="NeverCreated"]'),
    ).toHaveCount(0);
  });
});
