import { test } from "node:test";
import assert from "node:assert/strict";
import type {
	AgentConnection,
	AgentConnectionHeartbeat,
	AgentConnectionModelCatalog,
	AgentConnectionResourceSnapshot,
} from "../agent-connection/types.js";
import { InteractiveMode } from "./interactive-mode.js";

type Deferred<T> = {
	promise: Promise<T>;
	resolve(value: T): void;
};

function deferred<T>(): Deferred<T> {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((done) => {
		resolve = done;
	});
	return { promise, resolve };
}

test("late model and resource snapshots from a replaced session are ignored", async () => {
	const catalog = deferred<AgentConnectionModelCatalog>();
	const resources = deferred<AgentConnectionResourceSnapshot>();
	const connection = {
		getModelCatalog: () => catalog.promise,
		getResourceSnapshot: () => resources.promise,
	} as unknown as AgentConnection;
	const mode = Object.create(InteractiveMode.prototype) as {
		agentConnection: AgentConnection;
		sessionEventGeneration: number;
		connectionModelsFetchedAt: number;
		connectionResourceSnapshot: AgentConnectionResourceSnapshot | undefined;
		applyConnectionModelCatalog(catalog: AgentConnectionModelCatalog): void;
		refreshConnectionCatalogModelAndResources(connection: AgentConnection, generation: number): Promise<void>;
	};
	let appliedCatalog = false;
	const previousResources = { marker: "previous" } as unknown as AgentConnectionResourceSnapshot;
	Object.assign(mode, {
		agentConnection: connection,
		sessionEventGeneration: 3,
		connectionModelsFetchedAt: 17,
		connectionResourceSnapshot: previousResources,
		applyConnectionModelCatalog: () => {
			appliedCatalog = true;
		},
	});

	const refresh = mode.refreshConnectionCatalogModelAndResources(connection, 3);
	mode.sessionEventGeneration = 4;
	catalog.resolve({ models: [], configuredProviders: [] });
	resources.resolve({} as AgentConnectionResourceSnapshot);
	await refresh;

	assert.equal(appliedCatalog, false);
	assert.equal(mode.connectionModelsFetchedAt, 17);
	assert.equal(mode.connectionResourceSnapshot, previousResources);
});



test("late same-session catalog results cannot overwrite a newer auth refresh", async () => {
	const catalog = deferred<AgentConnectionModelCatalog>();
	const resources = deferred<AgentConnectionResourceSnapshot>();
	const connection = {
		getModelCatalog: () => catalog.promise,
		getResourceSnapshot: () => resources.promise,
	} as unknown as AgentConnection;
	const mode = Object.create(InteractiveMode.prototype) as {
		agentConnection: AgentConnection;
		sessionEventGeneration: number;
		connectionModelsRefreshVersion: number;
		connectionModelsFetchedAt: number;
		connectionResourceSnapshot: AgentConnectionResourceSnapshot | undefined;
		applyConnectionModelCatalog(catalog: AgentConnectionModelCatalog): void;
		refreshConnectionCatalogModelAndResources(connection: AgentConnection, generation: number): Promise<void>;
	};
	let appliedCatalog = false;
	const previousResources = { marker: "previous" } as unknown as AgentConnectionResourceSnapshot;
	const currentResources = { marker: "latest" } as unknown as AgentConnectionResourceSnapshot;
	Object.assign(mode, {
		agentConnection: connection,
		sessionEventGeneration: 5,
		connectionModelsRefreshVersion: 7,
		connectionModelsFetchedAt: 17,
		connectionResourceSnapshot: previousResources,
		applyConnectionModelCatalog: () => {
			appliedCatalog = true;
		},
	});

	const refresh = mode.refreshConnectionCatalogModelAndResources(connection, 5);
	mode.connectionModelsRefreshVersion = 8;
	catalog.resolve({ models: [], configuredProviders: ["stale-provider"] });
	resources.resolve(currentResources);
	await refresh;

	assert.equal(appliedCatalog, false);
	assert.equal(mode.connectionModelsFetchedAt, 17);
	assert.equal(mode.connectionResourceSnapshot, currentResources);
});

test("late roster subscriptions are disposed instead of rebinding to a replaced session", async () => {
	const roster = deferred<{ summaries(): []; dispose(): Promise<void> }>();
	let disposed = 0;
	const connection = {
		subscribeAgentRoster: () => roster.promise,
	} as unknown as AgentConnection;
	const mode = Object.create(InteractiveMode.prototype) as {
		agentConnection: AgentConnection;
		sessionEventGeneration: number;
		rosterBar: { summaries(): []; dispose(): Promise<void> } | undefined;
		updateSubagentSummaryLine(): void;
		subscribeToRosterBar(): Promise<void>;
	};
	Object.assign(mode, {
		agentConnection: connection,
		sessionEventGeneration: 9,
		rosterBar: undefined,
		updateSubagentSummaryLine: () => {},
	});

	const subscribe = mode.subscribeToRosterBar();
	mode.sessionEventGeneration = 10;
	roster.resolve({
		summaries: () => [],
		dispose: async () => {
			disposed++;
		},
	});
	await subscribe;

	assert.equal(mode.rosterBar, undefined);
	assert.equal(disposed, 1);
});


test("heartbeat refreshes do not reuse or apply a prior session's in-flight result", async () => {
	const sessionA = deferred<AgentConnectionHeartbeat[]>();
	const sessionB = deferred<AgentConnectionHeartbeat[]>();
	let listCalls = 0;
	const connection = {
		listHeartbeats: () => (listCalls++ === 0 ? sessionA.promise : sessionB.promise),
	} as unknown as AgentConnection;
	const mode = Object.create(InteractiveMode.prototype) as {
		agentConnection: AgentConnection;
		sessionEventGeneration: number;
		heartbeatRefreshPromise: Promise<void> | undefined;
		heartbeatRefreshGeneration: number | undefined;
		heartbeatRefreshRequested: boolean;
		appliedHeartbeats: AgentConnectionHeartbeat[][];
		applyHeartbeatCatalog(heartbeats: AgentConnectionHeartbeat[]): void;
		refreshHeartbeatCatalog(): Promise<void>;
	};
	Object.assign(mode, {
		agentConnection: connection,
		sessionEventGeneration: 20,
		heartbeatRefreshPromise: undefined,
		heartbeatRefreshGeneration: undefined,
		heartbeatRefreshRequested: false,
		appliedHeartbeats: [],
		applyHeartbeatCatalog(heartbeats: AgentConnectionHeartbeat[]) {
			mode.appliedHeartbeats.push(heartbeats);
		},
	});

	const refreshA = mode.refreshHeartbeatCatalog();
	mode.sessionEventGeneration = 21;
	const refreshB = mode.refreshHeartbeatCatalog();
	assert.equal(listCalls, 2, "new session starts its own heartbeat read");

	sessionA.resolve([{ id: "heartbeat-A" }] as unknown as AgentConnectionHeartbeat[]);
	await refreshA;
	assert.deepEqual(mode.appliedHeartbeats, []);
	sessionB.resolve([{ id: "heartbeat-B" }] as unknown as AgentConnectionHeartbeat[]);
	await refreshB;
	assert.deepEqual(mode.appliedHeartbeats, [[{ id: "heartbeat-B" }]]);
});
