import { describe, expect, test } from "bun:test"
import webConfig from "../src/renderer/vite.web.config"

type ViteWorkerConfig = {
	worker?: {
		format?: string
	}
}

describe("desktop Vite config", () => {
	test("keeps standalone web workers in ESM format", () => {
		expect((webConfig as ViteWorkerConfig).worker).toEqual({ format: "es" })
	})
})
