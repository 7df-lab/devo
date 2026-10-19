export const PRIME_INFERENCE_PROVIDER_ID = "prime-inference";
export const PRIME_INFERENCE_BASE_URL = "";
export function buildPrimeInferenceModels<T>(..._args: unknown[]): T[] { return []; }
export function mergePrimeInferenceModels<T>(models: T[] = []): T[] { return models; }
export function readCachedPrimeInferenceModels(): unknown[] { return []; }
export class PrimeInferenceCatalogRequestError extends Error {}
export async function fetchPrimeInferenceModelCatalog(): Promise<unknown[]> { return []; }
export async function refreshPrimeInferenceModels(): Promise<void> {}
