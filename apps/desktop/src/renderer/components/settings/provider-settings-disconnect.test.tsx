import { afterEach, expect, mock, test } from "bun:test";
import { Window } from "happy-dom";
import { act } from "react";
import type { Root } from "react-dom/client";

const dom = new Window({ url: "http://localhost", width: 1188, height: 848 });
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	HTMLInputElement: dom.HTMLInputElement,
	ShadowRoot: dom.ShadowRoot,
	Event: dom.Event,
	MouseEvent: dom.MouseEvent,
	KeyboardEvent: dom.KeyboardEvent,
	MutationObserver: dom.MutationObserver,
	ResizeObserver: dom.ResizeObserver,
	getComputedStyle: dom.getComputedStyle.bind(dom),
	requestAnimationFrame: dom.requestAnimationFrame.bind(dom),
	cancelAnimationFrame: dom.cancelAnimationFrame.bind(dom),
	IS_REACT_ACT_ENVIRONMENT: true,
});
const provider = { id: "local", name: "Local QA", enabled: true, models: {} };
const invalidate = mock(() => {});
const disconnect = mock((_params: unknown): Promise<void> => Promise.resolve());
mock.module("../../hooks/use-devo-data", () => ({
	useProviderCatalog: () => ({
		data: {
			providers: [provider],
			connectedIds: new Set([provider.id]),
			templateIds: new Set(),
			connectionModels: {},
		},
		loading: false,
		error: null,
		reload: () => {},
	}),
}));
mock.module("../../lib/invalidate-provider-queries", () => ({
	invalidateProviderDependentQueries: invalidate,
}));
mock.module("../../services/connection-manager", () => ({
	getBaseClient: () => ({ provider: { disconnect } }),
}));
mock.module("./desktop-oauth-actions", () => ({
	DesktopOAuthActions: () => null,
}));
mock.module("./connect-provider-dialog", () => ({
	ConnectProviderDialog: () => null,
}));
mock.module("./template-connect-dialog", () => ({
	TemplateConnectDialog: () => null,
}));
mock.module("./provider-icon", () => ({ ProviderIcon: () => null }));
const { createRoot } = await import("react-dom/client");
const { ProviderSettings } = await import("./provider-settings");
let root: Root | undefined;
let mount: HTMLElement;
async function click(label: string, scope: ParentNode = document) {
	const button = [...scope.querySelectorAll("button")].find(
		(entry) => entry.textContent?.trim() === label,
	);
	if (!button) throw new Error(`Missing ${label}`);
	await act(async () => button.click());
}
function getDialog() {
	const dialog = document.querySelector('[role="dialog"]');
	if (!dialog) throw new Error("Missing disconnect confirmation");
	return dialog;
}
async function render() {
	mount = document.createElement("div");
	document.body.append(mount);
	root = createRoot(mount);
	await act(async () => root?.render(<ProviderSettings />));
	await click("Disconnect");
}
afterEach(async () => {
	await act(async () => root?.unmount());
	mount?.remove();
	disconnect.mockReset();
	invalidate.mockClear();
});

test("pending disconnect cannot be dismissed and failure stays available for retry", async () => {
	let reject!: (error: Error) => void;
	disconnect.mockImplementationOnce(
		() =>
			new Promise<void>((_resolve, rejectRequest) => {
				reject = rejectRequest;
			}),
	);
	await render();
	const dialog = getDialog();
	await click("Disconnect", dialog);
	await click("Cancel", dialog);
	expect(document.querySelector('[role="dialog"]')?.textContent).toContain(
		"Disconnecting…",
	);
	expect(dialog.querySelector('[data-slot="dialog-close"]')).toBeNull();
	await act(async () => {
		dialog.dispatchEvent(
			new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
		);
	});
	expect(document.querySelector('[role="dialog"]')?.textContent).toContain(
		"Disconnecting…",
	);
	await act(async () => reject(new Error("Provider unavailable")));
	expect(document.querySelector('[role="alert"]')?.textContent).toBe(
		"Provider unavailable",
	);
	expect(invalidate).not.toHaveBeenCalled();
	disconnect.mockImplementationOnce(async () => {});
	await click("Disconnect", getDialog());
	expect(disconnect.mock.calls).toEqual([
		[{ providerId: "local" }],
		[{ providerId: "local" }],
	]);
	expect(invalidate).toHaveBeenCalledTimes(1);
	expect(document.querySelector('[role="dialog"]')).toBeNull();
});

test("cancel before disconnect sends no request", async () => {
	await render();
	await click("Cancel", getDialog());
	expect(disconnect).not.toHaveBeenCalled();
	expect(document.querySelector('[role="dialog"]')).toBeNull();
});

test("repeated confirmation starts one disconnect and closes only after success", async () => {
	let resolve!: () => void;
	disconnect.mockImplementationOnce(
		() =>
			new Promise<void>((finish) => {
				resolve = finish;
			}),
	);
	await render();
	const dialog = getDialog();
	const confirm = [...dialog.querySelectorAll("button")].find(
		(button) => button.textContent?.trim() === "Disconnect",
	);
	if (!confirm) throw new Error("Missing disconnect button");
	await act(async () => {
		confirm.click();
		confirm.click();
	});
	expect(disconnect.mock.calls).toEqual([[{ providerId: "local" }]]);
	expect(document.querySelector('[role="dialog"]')?.textContent).toContain(
		"Disconnecting…",
	);
	await act(async () => resolve());
	expect(invalidate).toHaveBeenCalledTimes(1);
	expect(document.querySelector('[role="dialog"]')).toBeNull();
});

test("outside press dismisses idle confirmation but not a pending disconnect", async () => {
	let resolve!: () => void;
	disconnect.mockImplementationOnce(
		() =>
			new Promise<void>((finish) => {
				resolve = finish;
			}),
	);
	await render();
	async function pressOutside() {
		const backdrop = document.querySelector('[data-slot="dialog-overlay"]');
		if (!backdrop) throw new Error("Missing dialog backdrop");
		await act(async () => {
			backdrop.dispatchEvent(
				new dom.PointerEvent("pointerdown", {
					bubbles: true,
					pointerType: "mouse",
					button: 0,
				}),
			);
			backdrop.dispatchEvent(
				new MouseEvent("mousedown", { bubbles: true, button: 0 }),
			);
			backdrop.dispatchEvent(
				new MouseEvent("mouseup", { bubbles: true, button: 0 }),
			);
			backdrop.dispatchEvent(
				new MouseEvent("click", { bubbles: true, button: 0 }),
			);
		});
	}
	await pressOutside();
	expect(document.querySelector('[role="dialog"]')).toBeNull();
	await click("Disconnect");
	await click("Disconnect", getDialog());
	await pressOutside();
	expect(getDialog().textContent).toContain("Disconnecting…");
	await act(async () => resolve());
	expect(document.querySelector('[role="dialog"]')).toBeNull();
});

test("close button dismisses idle confirmation without disconnecting", async () => {
	await render();
	await click("Close", getDialog());
	expect(disconnect).not.toHaveBeenCalled();
	expect(document.querySelector('[role="dialog"]')).toBeNull();
});

test("a failed disconnect from an unmounted page cannot change a new confirmation", async () => {
	let reject!: (error: Error) => void;
	disconnect.mockImplementationOnce(
		() =>
			new Promise<void>((_resolve, rejectRequest) => {
				reject = rejectRequest;
			}),
	);
	await render();
	await click("Disconnect", getDialog());
	await act(async () => root?.unmount());
	mount.remove();
	await render();
	await act(async () => reject(new Error("Obsolete disconnect failure")));
	expect(getDialog().textContent).toContain("Disconnect Local QA?");
	expect(document.querySelector('[role="alert"]')).toBeNull();
	expect(invalidate).not.toHaveBeenCalled();
	await click("Cancel", getDialog());
	expect(document.querySelector('[role="dialog"]')).toBeNull();
});
