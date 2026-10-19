export class VersionedRenderCache {
	private cachedWidth?: number;
	private cachedVersion?: number;
	private cachedLines?: string[];

	get(width: number, version: number): string[] | undefined {
		if (this.cachedWidth === width && this.cachedVersion === version) {
			return this.cachedLines;
		}
		return undefined;
	}

	set(width: number, version: number, lines: string[]): string[] {
		this.cachedWidth = width;
		this.cachedVersion = version;
		this.cachedLines = lines;
		return lines;
	}

	invalidate(): void {
		this.cachedWidth = undefined;
		this.cachedVersion = undefined;
		this.cachedLines = undefined;
	}
}
