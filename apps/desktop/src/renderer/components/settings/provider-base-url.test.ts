import { expect, test } from "bun:test"
import { isValidProviderBaseUrl } from "./provider-base-url"

test("accepts absolute HTTP(S) local and custom endpoints", () => {
	for (const url of [
		"http://localhost:11434",
		"http://127.0.0.1:1234/v1",
		"http://[::1]:8080/v1/",
		"https://gateway.example/custom/v4",
		" HTTP://localhost:11434/v1 ",
	]) {
		expect(isValidProviderBaseUrl(url)).toBe(true)
	}
})

test("rejects malformed endpoints and unsupported schemes", () => {
	for (const url of [
		"",
		"not-a-url",
		"localhost:11434/v1",
		"http:localhost",
		"http://",
		"https://bad host/v1",
		"ftp://localhost/v1",
		"file:///tmp/local-model",
		"http://localhost:70000/v1",
	]) {
		expect(isValidProviderBaseUrl(url)).toBe(false)
	}
})
