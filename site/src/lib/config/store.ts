import { invoke } from "@tauri-apps/api/core";
import { Resource } from "../data/tauri.svelte";

export type TranslationProvider =
  | "google"
  | "openai"
  | "deepseek"
  | "zai"
  | "openrouter";

export type Model = {
  id: string;
  name: string;
  provider?: TranslationProvider;
};

export type ProviderMeta = {
  id: TranslationProvider;
  name: string;
  defaultModel: string;
  apiKeyField:
    | "geminiApiKey"
    | "openaiApiKey"
    | "deepseekApiKey"
    | "zaiApiKey"
    | "openrouterApiKey";
  modelSelection?: "flat" | "family";
};

export type Language = {
  id: string;
  name: string;
  localName?: string;
};

export type Config = {
  targetLanguageId?: string;
  translationProvider: TranslationProvider;
  geminiApiKey?: string;
  openaiApiKey?: string;
  deepseekApiKey?: string;
  zaiApiKey?: string;
  openrouterApiKey?: string;
  model: string;
  translationConcurrency?: number;
  spotifyClientId?: string;
  spotifyPreloadCount?: number;
  spotifyShowNextTrack?: boolean;
  ankiEndpoint?: string;
  ankiApiKey?: string;
  syncEnabled?: boolean;
  syncDeviceName?: string;
  tapToRevealTranslations?: boolean;
};

export function modelsForDropdown(
  models: Model[],
  provider: TranslationProvider,
  selectedId: string,
): { list: Model[]; orphan: boolean } {
  const list = models.filter((m) => m.provider === provider);
  if (selectedId !== "" && !list.some((m) => m.id === selectedId)) {
    return {
      list: [{ id: selectedId, name: selectedId, provider }, ...list],
      orphan: true,
    };
  }
  return { list, orphan: false };
}

/** Provider change or empty selection → defaultModel. Same-provider orphans stay. */
export function resolveModelSelection(
  previousProvider: TranslationProvider | undefined,
  provider: TranslationProvider,
  selectedId: string,
  defaultModel: string,
): string {
  const providerChanged =
    previousProvider !== undefined && previousProvider !== provider;
  if (!selectedId || providerChanged) {
    return defaultModel;
  }
  return selectedId;
}

export function openRouterFamilyFromModelId(id: string): string {
  const normalized = id.startsWith("~") ? id.slice(1) : id;
  const slash = normalized.indexOf("/");
  if (slash <= 0) {
    return "other";
  }
  return normalized.slice(0, slash);
}

export function openRouterFamilies(models: Model[]): string[] {
  const families = new Set(
    models.map((m) => openRouterFamilyFromModelId(m.id)),
  );
  return [...families].sort((a, b) =>
    formatOpenRouterFamilyLabel(a).localeCompare(
      formatOpenRouterFamilyLabel(b),
    ),
  );
}

export function openRouterModelsInFamily(
  models: Model[],
  family: string,
): Model[] {
  return models.filter((m) => openRouterFamilyFromModelId(m.id) === family);
}

export function resolveOpenRouterFamily(
  family: string,
  modelId: string,
  families: string[],
): string {
  if (families.includes(family)) {
    return family;
  }
  const fromModel = openRouterFamilyFromModelId(modelId);
  if (families.includes(fromModel)) {
    return fromModel;
  }
  return families[0] ?? "other";
}

export function formatOpenRouterFamilyLabel(family: string): string {
  if (family === "other") {
    return "Other";
  }
  return family
    .split("-")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

export function apiKeyForProvider(
  config: Config | undefined,
  providerMeta: ProviderMeta[] | undefined,
): string | undefined {
  if (!config) {
    return undefined;
  }
  const meta = providerMeta?.find((p) => p.id === config.translationProvider);
  if (!meta) {
    return config.geminiApiKey;
  }
  return config[meta.apiKeyField];
}

export function hasApiKeyForProvider(
  config: Config | undefined,
  providerMeta: ProviderMeta[] | undefined,
): boolean {
  const key = apiKeyForProvider(config, providerMeta);
  return !!key?.trim();
}

export async function getModels(): Promise<Model[]> {
  let models = await invoke<Model[]>("get_models");
  return models;
}

export async function getTranslationProviders(): Promise<ProviderMeta[]> {
  let providers = await invoke<ProviderMeta[]>("get_translation_providers");
  return providers;
}

export async function getLanguages() {
  let languages = await invoke<Language[]>("get_languages");
  return languages;
}

export async function parseLanguageId(code: string): Promise<string | null> {
  return invoke<string | null>("parse_language_id", { code });
}

export async function setConfig(config: Config) {
  await invoke("update_config", { config: config });
}

export async function getConfig() {
  return await invoke<Config>("get_config");
}

export async function purgeGeminiCaches(): Promise<number> {
  return await invoke<number>("purge_gemini_caches");
}

export const configStore = new Resource<Config>("get_config", {}, [
  { name: "config_updated", filter: () => true },
]);
export const models = new Resource<Model[]>("get_models", {}, [], []);
