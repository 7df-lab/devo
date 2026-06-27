import { describe, expect, test } from "bun:test"
import { readFile } from "node:fs/promises"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const sourcePath = join(dirname(fileURLToPath(import.meta.url)), "sidebar-layout.tsx")

describe("sidebar layout window controls", () => {
	test("sidebar toggle has an accessible name", async () => {
		const source = await readFile(sourcePath, "utf8")

		expect(source).toContain('aria-label="Toggle sidebar"')
	})
})
