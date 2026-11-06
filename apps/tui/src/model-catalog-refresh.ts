type CatalogRpc = {
  request(method: string, params: Record<string, unknown>): Promise<unknown>;
};

/** Coalesce background checks and back off after failures; the server owns catalog TTLs. */
export class ModelCatalogRefresh {
  private pending: Promise<void> | undefined;
  private checkedAt: number | undefined;

  constructor(private readonly rpc: CatalogRpc, private readonly now = Date.now) {}

  refresh(): Promise<void> {
    if (this.pending) return this.pending;
    if (this.checkedAt !== undefined && this.now() - this.checkedAt < 60_000) return Promise.resolve();
    this.pending = this.update().catch(() => undefined).finally(() => {
      this.checkedAt = this.now();
      this.pending = undefined;
    });
    return this.pending;
  }

  private async update(): Promise<void> {
    await this.rpc.request("model/catalog/refresh", { policy: "ifStale" });
  }
}
