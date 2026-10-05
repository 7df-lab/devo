import type { AutocompleteItem } from "@earendil-works/pi-tui";
import { fuzzyFilter } from "@earendil-works/pi-tui";

export interface ModelAutocompleteCandidate {
	id: string;
	provider: string;
}

export function getModelArgumentCompletions(
	prefix: string,
	models: ReadonlyArray<ModelAutocompleteCandidate>,
): AutocompleteItem[] | null {
	if (models.length === 0) return null;

	const items = models.map((model) => ({
		id: model.id,
		provider: model.provider,
		label: `${model.provider}/${model.id}`,
	}));
	const filtered = fuzzyFilter(items, prefix, (item) => `${item.id} ${item.provider}`);
	if (filtered.length === 0) return null;

	return filtered.map((item) => ({
		value: item.label,
		label: item.id,
		description: item.provider,
	}));
}
