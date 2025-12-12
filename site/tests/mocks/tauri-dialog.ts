/**
 * Mock implementation of @tauri-apps/plugin-dialog for Playwright tests.
 */

export type OpenDialogOptions = {
  defaultPath?: string;
  directory?: boolean;
  multiple?: boolean;
  filters?: Array<{
    name: string;
    extensions: string[];
  }>;
  title?: string;
};

export type SaveDialogOptions = {
  defaultPath?: string;
  filters?: Array<{
    name: string;
    extensions: string[];
  }>;
  title?: string;
};

export type MessageDialogOptions = {
  title?: string;
  kind?: 'info' | 'warning' | 'error';
  okLabel?: string;
};

export type ConfirmDialogOptions = {
  title?: string;
  kind?: 'info' | 'warning' | 'error';
  okLabel?: string;
  cancelLabel?: string;
};

/**
 * Opens a file/directory selection dialog.
 * In test mode, returns a mock path.
 */
export async function open(options?: OpenDialogOptions): Promise<string | string[] | null> {
  console.log('[Tauri Dialog Mock] open:', options);

  if (options?.directory) {
    return '/mock/selected/directory';
  }

  if (options?.multiple) {
    return ['/mock/file1.txt', '/mock/file2.txt'];
  }

  return '/mock/selected/file.txt';
}

/**
 * Opens a save file dialog.
 * In test mode, returns a mock path.
 */
export async function save(options?: SaveDialogOptions): Promise<string | null> {
  console.log('[Tauri Dialog Mock] save:', options);
  return '/mock/saved/file.txt';
}

/**
 * Shows a message dialog.
 * In test mode, immediately resolves.
 */
export async function message(message: string, options?: MessageDialogOptions): Promise<void> {
  console.log('[Tauri Dialog Mock] message:', message, options);
}

/**
 * Shows a confirmation dialog.
 * In test mode, returns true (confirmed).
 */
export async function ask(message: string, options?: ConfirmDialogOptions): Promise<boolean> {
  console.log('[Tauri Dialog Mock] ask:', message, options);
  return true;
}

/**
 * Shows a confirmation dialog with cancel option.
 * In test mode, returns true (confirmed).
 */
export async function confirm(message: string, options?: ConfirmDialogOptions): Promise<boolean> {
  console.log('[Tauri Dialog Mock] confirm:', message, options);
  return true;
}
