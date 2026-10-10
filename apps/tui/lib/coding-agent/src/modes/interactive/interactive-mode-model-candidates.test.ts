import { test } from "node:test";
import assert from "node:assert/strict";
import type { AgentConnection, AgentConnectionModel, AgentConnectionModelCatalog } from "../agent-connection/types.js";
import { InteractiveMode } from "./interactive-mode.js";

type ModelCandidateTestTarget = {
	connectionModelsFetchedAt: number;
	getModelCandidates(): Promise<AgentConnectionModel[]>;
	getScopedModelState(): Array<{ model: AgentConnectionModel }>;
	getAvailableConnectionModels(): AgentConnectionModel[];
	getConnectionAvailableModels(): Promise<AgentConnectionModel[]>;
};

const availableModels = [{ provider: "openai", id: "gpt-4o" }] as unknown as AgentConnectionModel[];

test("model candidates reuse a fresh connection catalog without another RPC", async () => {
	const mode = Object.create(InteractiveMode.prototype) as ModelCandidateTestTarget;
	let refreshCount = 0;
	mode.connectionModelsFetchedAt = Date.now();
	mode.getScopedModelState = () => [];
	mode.getAvailableConnectionModels = () => availableModels;
	mode.getConnectionAvailableModels = async () => {
		refreshCount++;
		return availableModels;
	};

	assert.deepEqual(await mode.getModelCandidates(), availableModels);
	assert.equal(refreshCount, 0);
});

test("model candidates refresh a missing connection catalog", async () => {
	const mode = Object.create(InteractiveMode.prototype) as ModelCandidateTestTarget;
	let refreshCount = 0;
	mode.connectionModelsFetchedAt = 0;
	mode.getScopedModelState = () => [];
	mode.getAvailableConnectionModels = () => availableModels;
	mode.getConnectionAvailableModels = async () => {
		refreshCount++;
		return availableModels;
	};

	assert.deepEqual(await mode.getModelCandidates(), availableModels);
	assert.equal(refreshCount, 1);
});

test("picker checks remote in the background and coalesces concurrent refreshes", async () => {
	const mode = Object.create(InteractiveMode.prototype) as {
		agentConnection: AgentConnection;
		connectionModelsRefreshVersion: number;
		connectionModelsFetchedAt: number;
		connectionModelCatalog: AgentConnectionModel[];
		connectionConfiguredProviders: Set<string>;
		getScopedModelState(): Array<{ model: AgentConnectionModel }>;
		getCachedModelCandidates(): AgentConnectionModel[];
		getModelSelectorRefreshPromise(options: { force: boolean }): Promise<AgentConnectionModel[]>;
	};
	const freshModels = [{ provider: "openai", id: "fresh-model" }] as unknown as AgentConnectionModel[];
	let complete!: (catalog: AgentConnectionModelCatalog) => void;
	let remoteChecks = 0;
	mode.agentConnection = {
		refreshModelCatalog: () => {
			remoteChecks++;
			return new Promise((resolve) => { complete = resolve; });
		},
		getModelCatalog: () => { throw new Error("Picker should check remote"); },
	} as unknown as AgentConnection;
	mode.connectionModelsRefreshVersion = 0;
	mode.connectionModelsFetchedAt = 0;
	mode.connectionModelCatalog = availableModels;
	mode.connectionConfiguredProviders = new Set(["openai"]);
	mode.getScopedModelState = () => [];
	const first = mode.getModelSelectorRefreshPromise({ force: true });
	const second = mode.getModelSelectorRefreshPromise({ force: true });
	assert.equal(remoteChecks, 1);
	assert.deepEqual(mode.getCachedModelCandidates(), availableModels);
	complete({ models: freshModels, configuredProviders: ["openai"] });
	assert.deepEqual(await Promise.all([first, second]), [freshModels, freshModels]);
});
