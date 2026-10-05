export interface Rgb {
	r: number;
	g: number;
	b: number;
}

export interface DefaultTerminalColors {
	foreground: Rgb;
	background: Rgb;
}

export type TerminalBackgroundKind = "dark" | "light";
export type TerminalColorMode = "truecolor" | "256color" | "ansi16" | "unknown";
export type OscColorKind = "foreground" | "background";

export interface OscColorResponse {
	kind: OscColorKind;
	rgb: Rgb;
}

export const QUERY_DEFAULT_FOREGROUND = "\x1b]10;?\x1b\\";
export const QUERY_DEFAULT_BACKGROUND = "\x1b]11;?\x1b\\";

const CUBE_VALUES = [0, 95, 135, 175, 215, 255];
const GRAY_VALUES = Array.from({ length: 24 }, (_, i) => 8 + i * 10);
const defaultColorListeners = new Set<() => void>();

let defaultTerminalColors: DefaultTerminalColors | undefined;

function clampChannel(value: number): number {
	return Math.max(0, Math.min(255, Math.round(value)));
}

function normalizeHexChannel(value: string): number | undefined {
	if (!/^[0-9a-fA-F]{2,4}$/.test(value)) {
		return undefined;
	}
	const parsed = parseInt(value, 16);
	if (Number.isNaN(parsed)) {
		return undefined;
	}
	if (value.length <= 2) {
		return parsed;
	}
	const max = 16 ** value.length - 1;
	return Math.round((parsed / max) * 255);
}

function parseRgbPayload(payload: string): Rgb | undefined {
	const rgbMatch = payload.match(/^rgb:([0-9a-fA-F]{2,4})\/([0-9a-fA-F]{2,4})\/([0-9a-fA-F]{2,4})$/);
	if (rgbMatch) {
		const r = normalizeHexChannel(rgbMatch[1]!);
		const g = normalizeHexChannel(rgbMatch[2]!);
		const b = normalizeHexChannel(rgbMatch[3]!);
		return r === undefined || g === undefined || b === undefined ? undefined : { r, g, b };
	}

	const hexMatch = payload.match(/^#?([0-9a-fA-F]{6})$/);
	if (!hexMatch) {
		return undefined;
	}
	const hex = hexMatch[1]!;
	return {
		r: parseInt(hex.slice(0, 2), 16),
		g: parseInt(hex.slice(2, 4), 16),
		b: parseInt(hex.slice(4, 6), 16),
	};
}

function colorDistance(a: Rgb, b: Rgb): number {
	const dr = a.r - b.r;
	const dg = a.g - b.g;
	const db = a.b - b.b;
	return dr * dr * 0.299 + dg * dg * 0.587 + db * db * 0.114;
}

function findClosestIndex(value: number, values: readonly number[]): number {
	let minDist = Infinity;
	let minIdx = 0;
	for (let i = 0; i < values.length; i++) {
		const dist = Math.abs(value - values[i]!);
		if (dist < minDist) {
			minDist = dist;
			minIdx = i;
		}
	}
	return minIdx;
}

function notifyDefaultColorListeners(): void {
	for (const listener of defaultColorListeners) {
		listener();
	}
}

export function rgbToHex(rgb: Rgb): string {
	const toHex = (value: number) => clampChannel(value).toString(16).padStart(2, "0");
	return `#${toHex(rgb.r)}${toHex(rgb.g)}${toHex(rgb.b)}`;
}

export function isLightColor(rgb: Rgb): boolean {
	return 0.299 * rgb.r + 0.587 * rgb.g + 0.114 * rgb.b > 128;
}

export function blendColor(top: Rgb, bottom: Rgb, alpha: number): Rgb {
	const clampedAlpha = Math.max(0, Math.min(1, alpha));
	return {
		r: clampChannel(top.r * clampedAlpha + bottom.r * (1 - clampedAlpha)),
		g: clampChannel(top.g * clampedAlpha + bottom.g * (1 - clampedAlpha)),
		b: clampChannel(top.b * clampedAlpha + bottom.b * (1 - clampedAlpha)),
	};
}

export function rgbTo256(rgb: Rgb): number {
	const rIdx = findClosestIndex(rgb.r, CUBE_VALUES);
	const gIdx = findClosestIndex(rgb.g, CUBE_VALUES);
	const bIdx = findClosestIndex(rgb.b, CUBE_VALUES);
	const cubeRgb = { r: CUBE_VALUES[rIdx]!, g: CUBE_VALUES[gIdx]!, b: CUBE_VALUES[bIdx]! };
	const cubeIndex = 16 + 36 * rIdx + 6 * gIdx + bIdx;
	const cubeDist = colorDistance(rgb, cubeRgb);

	const gray = Math.round(0.299 * rgb.r + 0.587 * rgb.g + 0.114 * rgb.b);
	const grayIdx = findClosestIndex(gray, GRAY_VALUES);
	const grayValue = GRAY_VALUES[grayIdx]!;
	const grayRgb = { r: grayValue, g: grayValue, b: grayValue };
	const grayIndex = 232 + grayIdx;
	const grayDist = colorDistance(rgb, grayRgb);

	const maxChannel = Math.max(rgb.r, rgb.g, rgb.b);
	const minChannel = Math.min(rgb.r, rgb.g, rgb.b);
	if (maxChannel - minChannel < 10 && grayDist < cubeDist) {
		return grayIndex;
	}

	return cubeIndex;
}

export function bestAnsiColor(rgb: Rgb, mode: TerminalColorMode): string | number {
	if (mode === "truecolor") {
		return rgbToHex(rgb);
	}
	if (mode === "256color") {
		return rgbTo256(rgb);
	}
	return "";
}

export function parseOscColorResponse(sequence: string): OscColorResponse | undefined {
	const match = sequence.match(/^\x1b\](10|11);([^\x07\x1b]+)(?:\x07|\x1b\\)$/);
	if (!match) {
		return undefined;
	}
	const rgb = parseRgbPayload(match[2]!);
	if (!rgb) {
		return undefined;
	}
	return {
		kind: match[1] === "10" ? "foreground" : "background",
		rgb,
	};
}

export function detectBackgroundFromColorFgBg(
	value: string | undefined = process.env.COLORFGBG,
): TerminalBackgroundKind | undefined {
	if (!value) {
		return undefined;
	}
	const parts = value.split(";");
	if (parts.length < 2) {
		return undefined;
	}
	const bg = parseInt(parts[1]!, 10);
	if (Number.isNaN(bg)) {
		return undefined;
	}
	return bg < 8 ? "dark" : "light";
}

export function getDefaultTerminalColors(): DefaultTerminalColors | undefined {
	return defaultTerminalColors;
}

export function setDefaultTerminalColors(colors: DefaultTerminalColors | undefined): void {
	defaultTerminalColors = colors;
	notifyDefaultColorListeners();
}

export function clearDefaultTerminalColors(): void {
	setDefaultTerminalColors(undefined);
}

export function getTerminalBackgroundKind(): TerminalBackgroundKind | undefined {
	const bg = defaultTerminalColors?.background;
	if (bg) {
		return isLightColor(bg) ? "light" : "dark";
	}
	return detectBackgroundFromColorFgBg();
}

export function onDefaultTerminalColorsChange(listener: () => void): () => void {
	defaultColorListeners.add(listener);
	return () => {
		defaultColorListeners.delete(listener);
	};
}
