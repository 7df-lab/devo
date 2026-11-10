import { afterAll, describe, expect, mock, test } from "bun:test";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Window } from "happy-dom";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { McpReconnectButton } from "./mcp-reconnect-button";

const dom = new Window({ url: "http://localhost" });
const original = Object.getOwnPropertyDescriptors(globalThis);
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	HTMLElement: dom.HTMLElement,
	Element: dom.Element,
	Node: dom.Node,
	IS_REACT_ACT_ENVIRONMENT: true,
});

async function mount(status = "failed", connected = true) {
	let resolve!: () => void;
	let reject!: (error: Error) => void;
	const reconnect = mock(
		(_params: { name: string }) =>
			new Promise<void>((yes, no) => {
				resolve = yes;
				reject = no;
			}),
	);
	const client = { mcp: { reconnect } };
	const queries = new QueryClient({
		defaultOptions: { queries: { retry: false } },
	});
	const invalidations: unknown[] = [];
	const invalidate = queries.invalidateQueries.bind(queries);
	queries.invalidateQueries = (filters) => {
		invalidations.push(filters);
		return invalidate(filters);
	};
	queries.setQueryData(["mcp-servers", "/project"], [{ name: "docs", status }]);
	queries.setQueryData(
		["mcp-tools", "/project", "docs"],
		[{ name: "old_tool" }],
	);
	const container = dom.document.createElement("div");
	const root = createRoot(container as unknown as HTMLElement);
	await act(async () => {
		root.render(
			<QueryClientProvider client={queries}>
				<McpReconnectButton
					client={client}
					connected={connected}
					directory="/project"
					server={{ name: "docs", status }}
				/>
			</QueryClientProvider>,
		);
	});
	return {
		container,
		reconnect,
		queries,
		invalidations,
		resolve: () => resolve(),
		reject: () => reject(new Error("transport unavailable")),
		async close() {
			await act(async () => root.unmount());
			queries.clear();
		},
	};
}

// React Query batches observer updates on a timer.
async function flush() {
	await new Promise((resolve) => setTimeout(resolve, 10));
}

describe("MCP reconnect control", () => {
	test("suppresses duplicate clicks, keeps cached status unchanged, and refreshes status and tools", async () => {
		const view = await mount();
		try {
			await act(async () => {
				view.container.querySelector("button")?.click();
				view.container.querySelector("button")?.click();
				await flush();
			});
			expect(view.reconnect.mock.calls).toEqual([[{ name: "docs" }]]);
			expect(view.container.querySelector("button")?.disabled).toBe(true);
			expect(view.container.textContent).toContain("Reconnecting…");
			expect(
				view.queries.getQueryData<Array<{ name: string; status: string }>>([
					"mcp-servers",
					"/project",
				]),
			).toEqual([{ name: "docs", status: "failed" }]);
			await act(async () => {
				view.resolve();
				await flush();
			});
			expect(view.invalidations).toEqual([
				{ queryKey: ["mcp-servers", "/project"] },
				{ queryKey: ["mcp-tools", "/project", "docs"] },
			]);
			expect(view.container.querySelector("button")?.disabled).toBe(false);
		} finally {
			await view.close();
		}
	});

	test("failure invalidates caches, surfaces an error, and allows a manual retry", async () => {
		const view = await mount();
		try {
			await act(async () => {
				view.container.querySelector("button")?.click();
				await flush();
			});
			await act(async () => {
				view.reject();
				await flush();
			});
			expect(view.invalidations).toEqual([
				{ queryKey: ["mcp-servers", "/project"] },
				{ queryKey: ["mcp-tools", "/project", "docs"] },
			]);
			expect(
				view.container.querySelector('[role="alert"]')?.textContent,
			).toContain("transport unavailable");
			expect(view.container.querySelector("button")?.disabled).toBe(false);
			await act(async () => {
				view.container.querySelector("button")?.click();
				await flush();
			});
			expect(view.reconnect.mock.calls).toEqual([
				[{ name: "docs" }],
				[{ name: "docs" }],
			]);
			await act(async () => {
				view.resolve();
				await flush();
			});
			expect(view.container.querySelector('[role="alert"]')).toBeNull();
		} finally {
			await view.close();
		}
	});

	for (const [status, connected] of [
		["starting", true],
		["disabled", true],
		["connected", false],
	] as const) {
		test(`blocks reconnect for status=${status}, connected=${connected}`, async () => {
			const view = await mount(status, connected);
			try {
				expect(view.container.querySelector("button")?.disabled).toBe(true);
				await act(async () => view.container.querySelector("button")?.click());
				expect(view.reconnect.mock.calls).toEqual([]);
			} finally {
				await view.close();
			}
		});
	}
});

afterAll(() => {
	for (const key of [
		"window",
		"document",
		"navigator",
		"HTMLElement",
		"Element",
		"Node",
		"IS_REACT_ACT_ENVIRONMENT",
	]) {
		if (original[key]) Object.defineProperty(globalThis, key, original[key]);
		else Reflect.deleteProperty(globalThis, key);
	}
	void dom.happyDOM.close();
});
