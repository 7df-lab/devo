type CatalogRpc = {
  request(method: string, params: Record<string, unknown>): Promise<unknown>;
};
type ModelInfo = Record<string, unknown>;

/** Authenticated discovery is explicit-login-only and never a persisted directory. */
export class ExplicitLoginModels {
  private readonly discovered = new Map<string, Record<string, ModelInfo>>();

  constructor(private readonly rpc: CatalogRpc) {}

  async discover(provider: string): Promise<void> {
    const result = await this.rpc.request("provider/discover", { providerId: provider, forceRefresh: true }) as {
      models?: Record<string, ModelInfo>;
    };
    if (!result.models || Object.keys(result.models).length === 0) {
      throw new Error(`Signed in to ${provider}, but no models were returned. Configure a model in Providers before selecting it.`);
    }
    this.discovered.set(provider, result.models);
  }

  clear(provider: string): void {
    this.discovered.delete(provider);
  }

  models(): ModelInfo[] {
    return [...this.discovered].flatMap(([provider, models]) =>
      Object.entries(models).map(([id, info]) => ({ ...info, id, provider })),
    );
  }

  /** Persist just the chosen transport entry before changing session settings. */
  async pinSelected(provider: string, modelId: string): Promise<void> {
    const info = this.discovered.get(provider)?.[modelId];
    if (!info) return;
    const listed = await this.rpc.request("provider/list", {}) as {
      providers?: ModelInfo[];
      connectionModels?: Record<string, Record<string, ModelInfo>>;
    };
    if (listed.connectionModels?.[provider]?.[modelId]) {
      delete this.discovered.get(provider)![modelId];
      return;
    }
    const connection = listed.providers?.find((entry) => entry.id === provider);
    if (!connection || !Array.isArray(connection.wireApis) || connection.wireApis.length === 0) {
      throw new Error(`Cannot select ${provider}/${modelId}: configure its provider connection first.`);
    }
    // Never copy discovered names, costs, limits, or capabilities into user config.
    const selected = {
      origin: "user",
      baseUrl: info.baseUrl ?? connection.baseUrl,
      wireApi: info.wireApi ?? connection.wireApis[0],
      requestModel: info.requestModel ?? modelId,
    };
    await this.rpc.request("provider/upsert", {
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
    delete this.discovered.get(provider)![modelId];
  }
}
