export interface PathMetadata {
	source: string;
	scope?: string;
	origin?: "package" | "top-level";
	baseDir?: string;
}
export interface ResolvedResource {
	path: string;
	enabled: boolean;
	metadata: PathMetadata;
}
export interface ResourceDiagnostic { [key: string]: unknown }
export interface ResolvedPaths {
	extensions: ResolvedResource[];
	skills: ResolvedResource[];
	prompts: ResolvedResource[];
	themes: ResolvedResource[];
	diagnostics: ResourceDiagnostic[];
}
export type MissingSourceAction = "install" | "skip" | "error";
export interface ProgressEvent { [key: string]: unknown }
export type ProgressCallback = (event: ProgressEvent) => void;
export interface PackageUpdate { source?: string; displayName: string; type?: string; scope?: string }
export interface ConfiguredPackage { [key: string]: unknown }
export interface PackageManager {
	resolve(): Promise<ResolvedPaths>;
}
const emptyResolved = (): ResolvedPaths => ({
	extensions: [],
	skills: [],
	prompts: [],
	themes: [],
	diagnostics: [],
});
export class DefaultPackageManager implements PackageManager {
	constructor(_options?: unknown) {}
	async checkForAvailableUpdates(): Promise<PackageUpdate[]> { return []; }
	async resolve(): Promise<ResolvedPaths> { return emptyResolved(); }
	async resolveExtensionSources(_paths?: unknown, _options?: unknown): Promise<ResolvedPaths> { return emptyResolved(); }
	async install(): Promise<void> {}
	async installAndPersist(): Promise<void> {}
}
