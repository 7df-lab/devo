import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const source = readFileSync(new URL("./prompt-toolbar.tsx", import.meta.url), "utf8")
const modelSelectorProps = source.match(/interface ModelSelectorProps \{[\s\S]*?\n\}/)?.[0] ?? ""
const promptToolbarProps = source.match(/export interface PromptToolbarProps \{[\s\S]*?\n\}/)?.[0] ?? ""

describe("Model selector menu", () => {
	test("does not expose a Last used presentation group", () => {
		expect({
			omitsLastUsedHeading: !source.includes("Last used"),
			omitsLastUsedPresentationState: !source.includes("lastUsedModels"),
			modelSelectorPropRemoved: !modelSelectorProps.includes("recentModels"),
			promptToolbarPropRemoved: !promptToolbarProps.includes("recentModels"),
		}).toEqual({
			omitsLastUsedHeading: true,
			omitsLastUsedPresentationState: true,
			modelSelectorPropRemoved: true,
			promptToolbarPropRemoved: true,
		})
	})

	test("shares the searchable model picker across composers", () => {
		const selector = readFileSync(new URL("./model-selector.tsx", import.meta.url), "utf8")
		const list = readFileSync(new URL("./model-picker-list.tsx", import.meta.url), "utf8")
		expect({
			sharedPicker: source.includes('import { ModelSelector } from "./model-selector"'),
			catalog: selector.includes("useProviderCatalog()"),
			search: list.includes("<SearchableListPopoverSearch"),
			activeModel: list.includes("model.value === activeValue"),
			providerIdentity: list.includes("model.providerName} · {model.modelID"),
		}).toEqual({
			sharedPicker: true, catalog: true, search: true, activeModel: true, providerIdentity: true,
		})
	})
})
