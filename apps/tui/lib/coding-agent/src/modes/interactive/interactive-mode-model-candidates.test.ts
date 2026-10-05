import { test } from "node:test";
import assert from "node:assert/strict";
import type { AgentConnectionModel } from "../agent-connection/types.js";
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
