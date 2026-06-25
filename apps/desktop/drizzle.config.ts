import { defineConfig } from "drizzle-kit"

export default defineConfig({
	schema: "./src/main/automation/schema.ts",
	out: "./drizzle",
	dialect: "sqlite",
})
