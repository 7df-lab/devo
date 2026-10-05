import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const switchFiles = [
	"general-settings.tsx",
	"mcp-settings.tsx",
	"model-edit-dialog.tsx",
	"model-settings.tsx",
	"notification-settings.tsx",
	"skill-settings.tsx",
].map((file) => ({
	file,
	source: readFileSync(new URL(`./${file}`, import.meta.url), "utf8"),
}))

describe("Settings switch accessibility", () => {
	for (const { file, source } of switchFiles) {
		test(`${file} gives every switch an accessible name`, () => {
			const switches = source.match(/<Switch\b[\s\S]*?\/>/g) ?? []
			expect(switches.length).toBeGreaterThan(0)
			for (const switchTag of switches) {
				expect(switchTag).toMatch(/aria-label=/)
			}
		})
	}
})

const settingsRowSource = readFileSync(new URL("./settings-row.tsx", import.meta.url), "utf8")

describe("SettingsRow label semantics", () => {
	test("uses a label only when a matching control ID is explicit", () => {
		expect(settingsRowSource).toContain("{htmlFor ? (")
		expect(settingsRowSource).toContain("<label htmlFor={htmlFor}")
		expect(settingsRowSource).toContain("<span className=\"text-sm font-normal tracking-tight text-foreground\">{label}</span>")
		expect(settingsRowSource).not.toContain("useId")
	})
})


const selectFiles = [
	"custom-provider-dialog.tsx",
	"general-settings.tsx",
	"model-edit-dialog.tsx",
	"notification-settings.tsx",
	"server-settings.tsx",
].map((file) => ({
	file,
	source: readFileSync(new URL(`./${file}`, import.meta.url), "utf8"),
}))

describe("Settings select accessibility", () => {
	for (const { file, source } of selectFiles) {
		test(`${file} labels each select trigger`, () => {
			const triggers = source.match(/<SelectTrigger\b[^>]*>/g) ?? []
			expect(triggers.length).toBeGreaterThan(0)
			for (const trigger of triggers) {
				expect(trigger).toMatch(/aria-label=|\bid=/)
			}
		})
	}
})

const modelEditDialogSource = readFileSync(new URL("./model-edit-dialog.tsx", import.meta.url), "utf8")

describe("Model editor advanced-control accessibility", () => {
	test("links field labels to their inputs", () => {
		expect(modelEditDialogSource).toContain("const inputId = useId()")
		expect(modelEditDialogSource).toContain("<Label htmlFor={inputId}")
		expect(modelEditDialogSource).toContain("<Input")
		expect(modelEditDialogSource).toContain("id={inputId}")
		expect(modelEditDialogSource).toContain("htmlFor={requestBodyId}")
		expect(modelEditDialogSource).toContain("id={requestBodyId}")
		expect(modelEditDialogSource).toContain('htmlFor="model-invocation-method"')
		expect(modelEditDialogSource).toContain('id="model-invocation-method"')
	})

	test("names dynamic header fields and exposes toggle state", () => {
		expect(modelEditDialogSource).toContain('aria-label={`Header ${index + 1} name`}')
		expect(modelEditDialogSource).toContain('aria-label={`Header ${index + 1} value`}')
		expect(modelEditDialogSource).toContain('aria-label={`Remove header ${index + 1}`}')
		expect(modelEditDialogSource).toContain("aria-pressed={active}")
		expect(modelEditDialogSource).toContain('role="group"')
	})

	test("exposes the advanced section state and name", () => {
		expect(modelEditDialogSource).toContain("aria-expanded={advancedOpen}")
		expect(modelEditDialogSource).toContain('role="region"')
		expect(modelEditDialogSource).toContain('aria-label="Advanced model settings"')
	})
})
