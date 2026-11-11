import type { AgentConnection, AgentConnectionHeartbeat, AgentConnectionShellTask } from "../agent-connection/types.js";

export type ActivitySnapshot<T> =
  | { status: "loading" | "unavailable" }
  | { status: "ready"; items: T[] }
  | { status: "error"; message: string };

/** Owns independent, non-overlapping source refreshes. Old sessions and stopped UIs cannot publish snapshots. */
export class ActivityMonitor {
  private generation = 0;
  private timers = new Set<ReturnType<typeof setTimeout>>();
  constructor(private readonly options: {
    getConnection: () => AgentConnection;
    loadHeartbeats?: () => Promise<AgentConnectionHeartbeat[]>;
    onHeartbeats: (snapshot: ActivitySnapshot<AgentConnectionHeartbeat>) => void;
    onShells: (snapshot: ActivitySnapshot<AgentConnectionShellTask>) => void;
    intervalMs?: number;
  }) {}

  start(): void {
    this.stop();
    const generation = this.generation;
    const connection = this.options.getConnection();
    this.options.onHeartbeats({ status: "loading" });
    this.options.onShells({ status: connection.listShellTasks ? "loading" : "unavailable" });
    void this.refresh(connection, generation, () => this.options.loadHeartbeats?.() ?? connection.listHeartbeats(), this.options.onHeartbeats);
    if (connection.listShellTasks) {
      void this.refresh(connection, generation, () => connection.listShellTasks!(), this.options.onShells);
    }
  }

  stop(): void {
    this.generation++;
    for (const timer of this.timers) clearTimeout(timer);
    this.timers.clear();
  }

  private async refresh<T>(
    connection: AgentConnection,
    generation: number,
    load: () => Promise<T[]>,
    publish: (snapshot: ActivitySnapshot<T>) => void,
  ): Promise<void> {
    const current = () => generation === this.generation && connection === this.options.getConnection();
    try {
      const items = await load();
      if (current()) publish({ status: "ready", items });
    } catch (error) {
      if (current()) publish({ status: "error", message: error instanceof Error ? error.message : String(error) });
    }
    if (!current()) return;
    const timer = setTimeout(() => {
      this.timers.delete(timer);
      void this.refresh(connection, generation, load, publish);
    }, this.options.intervalMs ?? 2_000);
    this.timers.add(timer);
    timer.unref?.();
  }
}
