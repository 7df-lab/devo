import { describe, expect, mock, test } from "bun:test"
import { createServer } from "node:net"

const openExternal = mock(async (_url: string) => {})
mock.module("electron", () => ({ shell: { openExternal } }))

const { loginDesktopOAuth } = await import("./provider-oauth")

function expectPortAvailable(port: number): Promise<void> {
	return new Promise((resolve, reject) => {
		const server = createServer()
		server.once("error", reject)
		server.listen(port, "127.0.0.1", () => server.close((error) => error ? reject(error) : resolve()))
	})
}

describe("desktop OAuth callback cancellation", () => {
	for (const providerId of ["openai-codex", "anthropic"] as const) {
		test(`${providerId} does not listen or open a browser when already cancelled`, async () => {
			openExternal.mockClear()
			const controller = new AbortController()
			controller.abort()

			await expect(
				loginDesktopOAuth(providerId, {
					signal: controller.signal,
					onUpdate: () => {},
				}),
			).rejects.toThrow("Login cancelled")
			expect(openExternal).not.toHaveBeenCalled()
		})

		test(`${providerId} releases callback port when browser launch fails and on retry`, async () => {
			openExternal.mockClear()
			const port = providerId === "openai-codex" ? 1455 : 53692
			openExternal.mockImplementationOnce(async () => {
				throw new Error("Browser launch failed")
			})
			await expect(loginDesktopOAuth(providerId, { onUpdate: () => {} })).rejects.toThrow(
				"Browser launch failed",
			)
			await expectPortAvailable(port)

			const controller = new AbortController()
			openExternal.mockImplementationOnce(async () => controller.abort())
			await expect(
				loginDesktopOAuth(providerId, {
					signal: controller.signal,
					onUpdate: () => {},
				}),
			).rejects.toThrow("Login cancelled")
			await expectPortAvailable(port)
			expect(openExternal).toHaveBeenCalledTimes(2)
		})

		test(`${providerId} skips the browser when cancelled on callback setup`, async () => {
			openExternal.mockClear()
			const controller = new AbortController()

			await expect(
				loginDesktopOAuth(providerId, {
					signal: controller.signal,
					onUpdate: () => controller.abort(),
				}),
			).rejects.toThrow("Login cancelled")
			expect(openExternal).not.toHaveBeenCalled()
		})
	}
})
