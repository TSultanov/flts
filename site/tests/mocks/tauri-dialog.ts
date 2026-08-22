/** Stand-in for @tauri-apps/plugin-dialog in Playwright tests. */

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
  kind?: "info" | "warning" | "error";
  okLabel?: string;
};

export type ConfirmDialogOptions = {
  title?: string;
  kind?: "info" | "warning" | "error";
  okLabel?: string;
  cancelLabel?: string;
};

export async function open(
  options?: OpenDialogOptions,
): Promise<string | string[] | null> {
  console.log("[Tauri Dialog Mock] open:", options);

  if (options?.directory) {
    return "/mock/selected/directory";
  }

  if (options?.multiple) {
    return ["/mock/file1.txt", "/mock/file2.txt"];
  }

  return "/mock/selected/file.txt";
}

export async function save(
  options?: SaveDialogOptions,
): Promise<string | null> {
  console.log("[Tauri Dialog Mock] save:", options);
  return "/mock/saved/file.txt";
}

export async function message(
  message: string,
  options?: MessageDialogOptions,
): Promise<void> {
  console.log("[Tauri Dialog Mock] message:", message, options);
}

export async function ask(
  message: string,
  options?: ConfirmDialogOptions,
): Promise<boolean> {
  console.log("[Tauri Dialog Mock] ask:", message, options);
  return true;
}

export async function confirm(
  message: string,
  options?: ConfirmDialogOptions,
): Promise<boolean> {
  console.log("[Tauri Dialog Mock] confirm:", message, options);
  return true;
}
