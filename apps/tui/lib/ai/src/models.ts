import type { Api, KnownProvider, Model, ModelThinkingLevel, Usage } from "./types.js";

// Devo resolves model definitions through the Native server (models.dev + user
// configuration). These legacy utility entrypoints intentionally have no hidden
// catalog or fallback; callers must supply a resolved model to provider adapters.
export function getModel(_provider: KnownProvider, _modelId: string): Model<Api> | undefined {
	return undefined;
}

export function getProviders(): KnownProvider[] {
	return [];
}

export function getModels(_provider: KnownProvider): Model<Api>[] {
	return [];
}

export function supportsFastMode<TApi extends Api>(model: Model<TApi>): boolean {
	const eligibleId =
		model.id === "gpt-5.4" || model.id === "gpt-5.5" || model.id === "gpt-5.6" || model.id.startsWith("gpt-5.6-");
	return (
		eligibleId &&
		((model.provider === "openai-codex" && model.api === "openai-codex-responses") ||
			(model.provider === "openai" && model.api === "openai-responses"))
	);
}

export interface CostOverrides {
	cacheWrite?: number;
}

export function calculateCost<TApi extends Api>(
	model: Model<TApi>,
	usage: Usage,
	overrides?: CostOverrides,
): Usage["cost"] {
	usage.cost.input = (model.cost.input / 1000000) * usage.input;
	usage.cost.output = (model.cost.output / 1000000) * usage.output;
	usage.cost.cacheRead = (model.cost.cacheRead / 1000000) * usage.cacheRead;
	usage.cost.cacheWrite = ((overrides?.cacheWrite ?? model.cost.cacheWrite) / 1000000) * usage.cacheWrite;
	usage.cost.total = usage.cost.input + usage.cost.output + usage.cost.cacheRead + usage.cost.cacheWrite;
	return usage.cost;
}

const EXTENDED_THINKING_LEVELS: ModelThinkingLevel[] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

export function getSupportedThinkingLevels<TApi extends Api>(model: Model<TApi>): ModelThinkingLevel[] {
	if (!model.reasoning) return ["off"];

	return EXTENDED_THINKING_LEVELS.filter((level) => {
		const mapped = model.thinkingLevelMap?.[level];
		if (mapped === null) return false;
		if (level === "xhigh" || level === "max") return mapped !== undefined;
		return true;
	});
}

export function clampThinkingLevel<TApi extends Api>(
	model: Model<TApi>,
	level: ModelThinkingLevel,
): ModelThinkingLevel {
	const availableLevels = getSupportedThinkingLevels(model);
	if (availableLevels.includes(level)) return level;

	const requestedIndex = EXTENDED_THINKING_LEVELS.indexOf(level);
	if (requestedIndex === -1) return availableLevels[0] ?? "off";

	for (let i = requestedIndex; i < EXTENDED_THINKING_LEVELS.length; i++) {
		const candidate = EXTENDED_THINKING_LEVELS[i];
		if (availableLevels.includes(candidate)) return candidate;
	}
	for (let i = requestedIndex - 1; i >= 0; i--) {
		const candidate = EXTENDED_THINKING_LEVELS[i];
		if (availableLevels.includes(candidate)) return candidate;
	}
	return availableLevels[0] ?? "off";
}

export function modelsAreEqual<TApi extends Api>(
	a: Model<TApi> | null | undefined,
	b: Model<TApi> | null | undefined,
): boolean {
	if (!a || !b) return false;
	return a.id === b.id && a.provider === b.provider;
}
