import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

async function makeArtifacts() {
	const root = await mkdtemp(join(tmpdir(), "devo-release-assets-"));
	const input = join(root, "artifacts");
	const output = join(root, "release-assets");
	const files = [];
	for (const arch of ["x64", "arm64"]) {
		const directory = join(input, `desktop-mac-${arch}`);
		await mkdir(directory, { recursive: true });
		const content = Buffer.from(`installer-${arch}`);
		const file = {
			url: `devo-desktop-0.2.0-mac-${arch}.zip`,
			sha512: createHash("sha512").update(content).digest("base64"),
			size: content.length,
		};
		files.push(file);
		await writeFile(join(directory, file.url), content);
		await writeFile(join(directory, `${file.url}.blockmap`), "blockmap");
		await writeFile(join(directory, "builder-debug.yml"), "debug: true\n");
		await writeFile(
			join(directory, "latest-mac.yml"),
			Bun.YAML.stringify({
				version: "0.2.0",
				files: [file],
				path: file.url,
				sha512: file.sha512,
			}),
		);
	}
	return { root, input, output, files };
}

async function collect(input: string, output: string) {
	const child = Bun.spawn({
		cmd: [process.execPath, join(import.meta.dir, "collect-release-assets.ts"), input, output, "v0.2.0"],
		stdout: "pipe",
		stderr: "pipe",
	});
	const [exitCode, stdout, stderr] = await Promise.all([
		child.exited,
		new Response(child.stdout).text(),
		new Response(child.stderr).text(),
	]);
	return { exitCode, stdout, stderr };
}

test("release collection preserves both Mac architectures and blockmaps without debug files", async () => {
	const fixture = await makeArtifacts();
	try {
		const result = await collect(fixture.input, fixture.output);
		expect(result).toEqual({
			exitCode: 0,
			stdout: "Collected 4 assets and 1 merged update manifests for v0.2.0\n",
			stderr: "",
		});
		expect({
			assets: (await readdir(fixture.output)).sort(),
			manifest: Bun.YAML.parse(await readFile(join(fixture.output, "latest-mac.yml"), "utf8")),
		}).toEqual({
			assets: [...fixture.files.flatMap(file => [file.url, `${file.url}.blockmap`]), "latest-mac.yml", "SHA256SUMS.txt"].sort(),
			manifest: {
				version: "0.2.0",
				files: fixture.files,
				path: fixture.files[0].url,
				sha512: fixture.files[0].sha512,
			},
		});
	} finally {
		await rm(fixture.root, { recursive: true, force: true });
	}
});

test("release collection retains online installation indexes and verifies every referenced pack", async () => {
	const fixture = await makeArtifacts();
	try {
		const directory = join(fixture.input, "online");
		await mkdir(directory);
		const entries = [];
		for (const kind of ["app", "node", "python"]) {
			const name = `devo-${kind}.tar.gz`;
			const content = Buffer.from(kind);
			await writeFile(join(directory, name), content);
			entries.push(`${kind} ${createHash("sha256").update(content).digest("hex")} ${name}`);
		}
		const name = "devo-tui-v0.2.0-x86_64-pc-windows-msvc.install.txt";
		await writeFile(join(directory, name), ["devo-install-v1", ...entries, ""].join("\n"));
		const result = await collect(fixture.input, fixture.output);
		expect({ exit: result.exitCode, stderr: result.stderr, index: await readFile(join(fixture.output, name), "utf8") }).toEqual({ exit: 0, stderr: "", index: ["devo-install-v1", ...entries, ""].join("\n") });
		await writeFile(join(directory, "devo-node.tar.gz"), "damaged");
		const corrupt = await collect(fixture.input, fixture.output);
		expect({ failed: corrupt.exitCode !== 0, checksumFailure: corrupt.stderr.includes("Runtime pack checksum or asset missing") }).toEqual({ failed: true, checksumFailure: true });
	} finally {
		await rm(fixture.root, { recursive: true, force: true });
	}
});

test("release collection rejects assets that disagree with update checksums", async () => {
	const fixture = await makeArtifacts();
	try {
		await writeFile(join(fixture.input, "desktop-mac-arm64", fixture.files[1].url), "installer-ARM64");
		const result = await collect(fixture.input, fixture.output);
		expect({
			exitCode: result.exitCode,
			checksumError: result.stderr.includes(`Update asset checksum mismatch: ${fixture.files[1].url}`),
		}).toEqual({ exitCode: 1, checksumError: true });
	} finally {
		await rm(fixture.root, { recursive: true, force: true });
	}
});

test("release collection includes devo-tui archives in SHA256SUMS", async () => {
	const fixture = await makeArtifacts();
	try {
		const name = "devo-tui-v0.2.0-x86_64-unknown-linux-musl.tar.gz";
		const content = Buffer.from("terminal bundle");
		await writeFile(join(fixture.input, name), content);
		const result = await collect(fixture.input, fixture.output);
		expect(result).toEqual({ exitCode: 0, stdout: "Collected 5 assets and 1 merged update manifests for v0.2.0\n", stderr: "" });
		expect(await readFile(join(fixture.output, name))).toEqual(content);
		const checksums = (await readFile(join(fixture.output, "SHA256SUMS.txt"), "utf8")).trim().split("\n");
		expect(checksums.filter(line => line.endsWith(`  ${name}`))).toEqual([`${createHash("sha256").update(content).digest("hex")}  ${name}`]);
	} finally {
		await rm(fixture.root, { recursive: true, force: true });
	}
});
