import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
	readWorkspacePackageVersion,
	syncCargoVersion,
} from "./sync-cargo-version";

async function tempDesktopDir(): Promise<string> {
	const repoRoot = await mkdtemp(join(tmpdir(), "devo-desktop-version-"));
	const desktopDir = join(repoRoot, "apps", "desktop");
	await mkdir(desktopDir, { recursive: true });
	return desktopDir;
}

describe("desktop Cargo version sync", () => {
	test("updates desktop package metadata from the workspace package version", async () => {
		const desktopDir = await tempDesktopDir();
		const repoRoot = join(desktopDir, "..", "..");
		await mkdir(repoRoot, { recursive: true });
		await writeFile(
			join(repoRoot, "Cargo.toml"),
			`
[workspace]
members = ["crates/cli"]

[workspace.package]
edition = "2024"
version = "0.1.21"
`,
		);
		await writeFile(
			join(desktopDir, "package.json"),
			`${JSON.stringify(
				{
					name: "@devo/desktop",
					productName: "Devo",
					version: "0.11.0",
				},
				null,
				"\t",
			)}\n`,
		);

		const result = await syncCargoVersion({ desktopDir });

		const packageJson = JSON.parse(
			await readFile(join(desktopDir, "package.json"), "utf8"),
		);
		expect(result).toEqual({
			changed: true,
			previousVersion: "0.11.0",
			version: "0.1.21",
		});
		expect(packageJson).toEqual({
			name: "@devo/desktop",
			productName: "Devo",
			version: "0.1.21",
		});
	});

	test("keeps the checked-in desktop package version aligned with Cargo", async () => {
		const scriptDir = dirname(fileURLToPath(import.meta.url));
		const desktopDir = join(scriptDir, "..");
		const cargoToml = await readFile(
			join(desktopDir, "..", "..", "Cargo.toml"),
			"utf8",
		);
		const packageJson = JSON.parse(
			await readFile(join(desktopDir, "package.json"), "utf8"),
		);

		expect(packageJson.version).toBe(readWorkspacePackageVersion(cargoToml));
	});
});
