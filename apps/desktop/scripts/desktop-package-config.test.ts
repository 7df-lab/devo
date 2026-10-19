import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { join } from "node:path"

const desktopDir = join(import.meta.dir, "..")

describe("desktop package runtime resources", () => {
	test("packages private runtime resources instead of public CLI launcher scripts", () => {
		const config = readFileSync(join(desktopDir, "electron-builder.yml"), "utf8")

		expect({
			includesPrivateRuntime: config.includes("from: resources/runtime"),
			excludesPublicCliLauncher: !config.includes("from: resources/bin"),
		}).toEqual({
			includesPrivateRuntime: true,
			excludesPublicCliLauncher: true,
		})
	})

	test("publishes desktop auto-update metadata to the canonical GitHub repository", () => {
		const config = readFileSync(join(desktopDir, "electron-builder.yml"), "utf8")
		const packageJson = JSON.parse(readFileSync(join(desktopDir, "package.json"), "utf8"))
		const releaseWorkflow = readFileSync(
			join(desktopDir, "..", "..", ".github", "workflows", "release.yml"),
			"utf8",
		)
		const updaterSource = readFileSync(join(desktopDir, "src", "main", "updater.ts"), "utf8")

		expect({
			homepage: packageJson.homepage,
			repositoryUrl: packageJson.repository.url,
			usesCanonicalOwner: config.includes("owner: 7df-lab"),
			usesCanonicalRepo: config.includes("repo: devo"),
			usesCanonicalReleasePage: updaterSource.includes("https://github.com/7df-lab/devo"),
			collectsReleaseAssets: releaseWorkflow.includes("bun apps/desktop/scripts/collect-release-assets.ts"),
			preservesMatrixArtifacts: !releaseWorkflow.includes("merge-multiple: true"),
		}).toEqual({
			homepage: "https://github.com/7df-lab/devo",
			repositoryUrl: "https://github.com/7df-lab/devo.git",
			usesCanonicalOwner: true,
			usesCanonicalRepo: true,
			usesCanonicalReleasePage: true,
			collectsReleaseAssets: true,
			preservesMatrixArtifacts: true,
		})
	})
})
