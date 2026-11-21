type CatalogRpc = {
  request(method: string, params: Record<string, unknown>): Promise<unknown>;
};
type ModelInfo = Record<string, unknown>;
const REASONING_FIELDS = [
  "reasoningCapability", "reasoning", "thinkingLevelMap", "reasoningImplementation",
  "defaultReasoningEffort", "defaultReasoningSelection",
] as const;

type Discovery = {
  models?: Record<string, ModelInfo>;
  pending?: Promise<void>;
  checkedAt?: number;
};
const DISCOVERY_COOLDOWN_MS = 60_000;

/** Account discoveries live in memory; only selected model routes are persisted. */
export class ExplicitLoginModels {
  private readonly discovered = new Map<string, Discovery>();

  constructor(private readonly rpc: CatalogRpc, private readonly now = Date.now) {}

  discover(provider: string, options: { force?: boolean } = {}): Promise<void> {
    const existing = this.discovered.get(provider);
    const force = options.force ?? true;
    if (!force) {
      if (existing?.pending) return existing.pending;
      if (existing?.checkedAt !== undefined && this.now() - existing.checkedAt < DISCOVERY_COOLDOWN_MS) {
        return Promise.resolve();
      }
    }
    // Each request owns its entry. A response arriving after logout or a new
    // login may finish, but it cannot put the old account's models back.
    const entry: Discovery = { models: existing?.models };
    this.discovered.set(provider, entry);
    const pending = this.fetchModels(provider, entry).finally(() => {
      entry.pending = undefined;
      entry.checkedAt = this.now();
    });
    entry.pending = pending;
    return pending;
  }

  private async fetchModels(provider: string, entry: Discovery): Promise<void> {
    // Use the client cooldown rather than a server cache shared by previous
    // credentials on the same provider route.
    const result = await this.rpc.request("provider/discover", { providerId: provider, forceRefresh: true }) as {
      models?: Record<string, ModelInfo>;
    };
    const models = Object.fromEntries(Object.entries(result.models ?? {}).filter(([, model]) => model.enabled !== false));
    entry.models = models;
    if (Object.keys(models).length === 0) {
      throw new Error(`Signed in to ${provider}, but no models were returned. Configure a model in Providers before selecting it.`);
    }
  }

  clear(provider: string): void {
    this.discovered.delete(provider);
  }

  models(): ModelInfo[] {
    return [...this.discovered].flatMap(([provider, { models }]) =>
      Object.entries(models ?? {}).map(([id, info]) => ({ ...info, id, provider })),
    );
  }

  /** Persist the chosen route and, for account discoveries, its effort settings. */
  async pinSelected(provider: string, modelId: string): Promise<void> {
    const discovered = this.discovered.get(provider)?.models;
    const info = discovered && Object.hasOwn(discovered, modelId) ? discovered[modelId] : undefined;
    const listed = await this.rpc.request("provider/list", {}) as {
      providers?: (ModelInfo & { models?: Record<string, ModelInfo> })[];
      connectedProviderIds?: string[];
      connectionModels?: Record<string, Record<string, ModelInfo>>;
    };
    const savedModels = listed.connectionModels?.[provider];
    const saved = savedModels && Object.hasOwn(savedModels, modelId) ? savedModels[modelId] : undefined;
    const connection = listed.providers?.find((entry) => entry.id === provider);
    const reasoning = Object.fromEntries(REASONING_FIELDS.flatMap((key) =>
      info?.[key] == null ? [] : [[key, info[key]]],
    ));
    const repairCodexPin = provider === "openai-codex" && saved?.origin === "user" &&
      saved.requestModel === modelId && info?.origin === "user" &&
      (reasoning.reasoningCapability != null || reasoning.thinkingLevelMap != null) &&
      REASONING_FIELDS.every((key) => saved[key] == null);
    if (saved && !repairCodexPin) {
      if (discovered) delete discovered[modelId];
      return;
    }
    if (!info) {
      // A public listing alone is not a Connection. Only pin catalog models
      // selected on a saved Connection; do not implicitly create one here.
      const catalogModels = connection?.models;
      if (!listed.connectedProviderIds?.includes(provider) || !catalogModels ||
        !Object.hasOwn(catalogModels, modelId) || catalogModels[modelId]?.enabled === false) return;
    }
    if (!connection || !Array.isArray(connection.wireApis) || connection.wireApis.length === 0) {
      throw new Error(`Cannot select ${provider}/${modelId}: configure its provider connection first.`);
    }
    // Public metadata must not redirect a saved credential. Use the pinned
    // provider route; explicit authenticated discovery can carry its own route.
    const baseUrl = info?.baseUrl ?? connection.baseUrl;
    if (!repairCodexPin && (typeof baseUrl !== "string" || !baseUrl.trim())) {
      throw new Error(`Cannot select ${provider}/${modelId}: configure its provider connection endpoint first.`);
    }
    const selected = repairCodexPin ? { ...saved, ...reasoning } : {
      origin: info ? "user" : "remote",
      baseUrl,
      wireApi: info?.wireApi ?? connection.wireApis[0],
      requestModel: info?.requestModel ?? modelId,
      ...reasoning,
    };
    await this.rpc.request("provider/upsert", {
      // A new selected row must be distinguished from read-only catalog rows
      // echoed by an editor. Repairing an existing row need not reset the default.
      ...(repairCodexPin ? {} : { defaultModel: `${provider}/${modelId}` }),
      provider: {
        ...connection,
        id: provider,
        name: connection.name ?? provider,
        baseUrl: connection.baseUrl,
        wireApis: connection.wireApis,
        enabled: connection.enabled ?? true,
        models: { [modelId]: selected },
        modelOverrides: {},
      },
    });
    // Future selections use the authoritative config, never stale discovery routes.
    if (discovered) delete discovered[modelId];
  }
}
