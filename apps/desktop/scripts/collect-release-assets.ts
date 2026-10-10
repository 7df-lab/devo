import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { copyFile, mkdir, readFile, readdir, stat, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";

type UpdateFile = {
	url: string;
	sha512: string;
	size?: number;
	[key: string]: unknown;
};

type UpdateManifest = {
	version: string;
	files: UpdateFile[];
	path?: string;
	[key: string]: unknown;
};

// Matrix jobs produce identically named update manifests. Keep their artifact
// directories separate until file lists have been merged across architectures.
const inputDir = resolve(Bun.argv[2] ?? "artifacts");
const outputDir = resolve(Bun.argv[3] ?? "release-assets");
const version = (Bun.argv[4] ?? "").replace(/^v/, "");
if (!version) throw new Error("Expected release tag as the third argument");

const assets = new Map<string, string>();
const manifests = new Map<string, UpdateManifest[]>();
const paths = Array.from(
	new Bun.Glob("**/*").scanSync({ cwd: inputDir, onlyFiles: true }),
).sort();

for (const relativePath of paths) {
	const name = basename(relativePath);
	const source = join(inputDir, relativePath);
	if (/^(latest|alpha|beta)(-[a-z0-9-]+)?\.yml$/.test(name)) {
		const manifest = Bun.YAML.parse(
			await readFile(source, "utf8"),
		) as UpdateManifest;
		if (
			manifest.version !== version ||
			!Array.isArray(manifest.files) ||
			!manifest.files.length
		) {
			throw new Error(`Invalid update manifest or version: ${relativePath}`);
		}
		const group = manifests.get(name) ?? [];
		group.push(manifest);
		manifests.set(name, group);
	} else if (/\.(AppImage|blockmap|deb|dmg|exe|rpm|tar\.gz|zip)$/.test(name)) {
		if (assets.has(name)) throw new Error(`Duplicate release asset: ${name}`);
		assets.set(name, source);
	}
}

if (!assets.size || !manifests.size) {
	throw new Error("Release assets or update manifests are missing");
}
await mkdir(outputDir, { recursive: true });
for (const [name, source] of assets) await copyFile(source, join(outputDir, name));

const checksums = new Map<string, string>();
for (const [name, group] of manifests) {
	// Preserve the x64 fallback for older clients. Modern Mac clients select the
	// appropriate architecture from the merged files list.
	group.sort(
		(a, b) =>
			Number(a.path?.includes("arm64") ?? false) -
			Number(b.path?.includes("arm64") ?? false),
	);
	const files = new Map<string, UpdateFile>();
	for (const manifest of group) {
		for (const file of manifest.files) {
			if (typeof file.url !== "string" || typeof file.sha512 !== "string") {
				throw new Error(`Invalid file entry in ${name}`);
			}
			const previous = files.get(file.url);
			if (previous && (previous.sha512 !== file.sha512 || previous.size !== file.size)) {
				throw new Error(`Conflicting update metadata: ${file.url}`);
			}
			const source = assets.get(file.url);
			if (!source) {
				throw new Error(`Update metadata references a missing asset: ${file.url}`);
			}
			if (file.size !== undefined && (await stat(source)).size !== file.size) {
				throw new Error(`Update asset size mismatch: ${file.url}`);
			}
			let checksum = checksums.get(file.url);
			if (!checksum) {
				const hash = createHash("sha512");
				for await (const chunk of createReadStream(source)) hash.update(chunk);
				checksum = hash.digest("base64");
				checksums.set(file.url, checksum);
			}
			if (checksum !== file.sha512) {
				throw new Error(`Update asset checksum mismatch: ${file.url}`);
			}
			files.set(file.url, file);
		}
	}
	const merged = { ...group[0], files: Array.from(files.values()) };
	await writeFile(join(outputDir, name), Bun.YAML.stringify(merged));
}

const sha256Lines: string[] = [];
for (const name of (await readdir(outputDir)).sort()) {
	if (name === "SHA256SUMS.txt") continue;
	const hash = createHash("sha256");
	for await (const chunk of createReadStream(join(outputDir, name))) hash.update(chunk);
	sha256Lines.push(`${hash.digest("hex")}  ${name}`);
}
await writeFile(join(outputDir, "SHA256SUMS.txt"), sha256Lines.join("\n") + "\n");

console.log(
	`Collected ${assets.size} assets and ${manifests.size} merged update manifests for v${version}`,
);
