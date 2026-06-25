import { describe, expect, test } from "bun:test"
import { formatShortcut } from "./shortcut-display"

describe("formatShortcut", () => {
	test("uses macOS symbols without separators", () => {
		expect(formatShortcut(["mod", "N"], "darwin")).toBe("⌘N")
		expect(formatShortcut(["mod", "B"], "darwin")).toBe("⌘B")
		expect(formatShortcut(["mod", "J"], "darwin")).toBe("⌘J")
		expect(formatShortcut(["shift", "mod", "D"], "darwin")).toBe("⇧⌘D")
		expect(formatShortcut(["mod", "Z"], "darwin")).toBe("⌘Z")
		expect(formatShortcut(["shift", "mod", "Z"], "darwin")).toBe("⇧⌘Z")
	})

	test("uses Ctrl text shortcuts on Windows", () => {
		expect(formatShortcut(["mod", "N"], "win32")).toBe("Ctrl+N")
		expect(formatShortcut(["mod", "B"], "win32")).toBe("Ctrl+B")
		expect(formatShortcut(["mod", "J"], "win32")).toBe("Ctrl+J")
		expect(formatShortcut(["shift", "mod", "D"], "win32")).toBe("Ctrl+Shift+D")
		expect(formatShortcut(["mod", "Z"], "win32")).toBe("Ctrl+Z")
		expect(formatShortcut(["shift", "mod", "Z"], "win32")).toBe("Ctrl+Shift+Z")
	})

	test("uses Ctrl text shortcuts on Linux and unknown platforms", () => {
		expect(formatShortcut(["mod", "N"], "linux")).toBe("Ctrl+N")
		expect(formatShortcut(["mod", "B"], "linux")).toBe("Ctrl+B")
		expect(formatShortcut(["mod", "J"], "linux")).toBe("Ctrl+J")
		expect(formatShortcut(["shift", "mod", "D"], "linux")).toBe("Ctrl+Shift+D")
		expect(formatShortcut(["mod", "Z"], "linux")).toBe("Ctrl+Z")
		expect(formatShortcut(["shift", "mod", "Z"], "linux")).toBe("Ctrl+Shift+Z")
		expect(formatShortcut(["mod", "N"], "freebsd")).toBe("Ctrl+N")
	})
})
