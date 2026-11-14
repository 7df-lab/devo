/** Keeps explicit logout and missing pinned credentials authoritative over stale session snapshots. */
export class ProviderLogoutState {
  private readonly loggedOut = new Set<string>();
  private missing = new Set<string>();
  private generations = new Map<string, number>();
  generation(provider: string): number { return this.generations.get(provider) ?? 0; }
  isLoggedOut(provider: string): boolean { return this.loggedOut.has(provider); }

  logout(provider: string): void { this.generations.set(provider, this.generation(provider) + 1); this.loggedOut.add(provider); }
  login(provider: string): void { this.generations.set(provider, this.generation(provider) + 1); this.loggedOut.delete(provider); this.missing.delete(provider); }
  unavailable(provider: string): boolean { return this.loggedOut.has(provider) || this.missing.has(provider); }

  reconcile(providers: Array<{ id?: string; name?: string; credential?: string | null }>, credentials: Array<{ id?: string }>): void {
    const ids = new Set(credentials.map((entry) => entry.id));
    this.missing = new Set(providers.filter((provider) => {
      const id = provider.credential?.trim();
      if (!id) return false;
      // Match Native resolve_provider_credential_id, including migrate-on-read
      // aliases. Never infer credentials belonging to an unrelated provider.
      const providerId = String(provider.id ?? provider.name ?? "");
      const normalized = providerId.trim().replace(/[^a-zA-Z0-9:-]/g, "_").toLowerCase().replace(/^_+|_+$/g, "");
      const migrated = id.replace(/(.+)(_api_key|_oauth)$/, "$1");
      return ![id, migrated, providerId, normalized, `${providerId.replace(/[\\/]/g, "_")}_oauth`, `${normalized || "provider"}_api_key`].some(candidate => ids.has(candidate));
    }).map((provider) => String(provider.id ?? provider.name ?? "")));
  }
}
