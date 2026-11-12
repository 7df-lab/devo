import { describe, expect, test } from "bun:test"
import { ReferenceSearchSession, type ReferenceSearchSnapshot } from "./reference-search-session"

function snapshot(id: string, query: string, complete = true): ReferenceSearchSnapshot {
	return {
		search_id: id,
		query,
		results: [
			{
				kind: "file",
				display_name: `${query}.txt`,
				insert_text: `${query}.txt`,
			},
		],
		total_file_match_count: 1,
		scanned_file_count: 1,
		file_search_complete: complete,
	}
}
function harness() {
	const requests: Array<{
		method: string
		params: any
		resolve: (value: unknown) => void
	}> = []
	const session = new ReferenceSearchSession(
		(method, params) =>
			new Promise((resolve) => {
				requests.push({ method, params, resolve })
			}),
		"/repo",
	)
	const seen: string[] = []
	session.subscribe((s) => seen.push(s.search_id + ":" + s.query))
	return { session, requests, seen }
}

describe("reference search request lifetimes", () => {
	test("late start cannot replace a newer search and cancels only its own id", async () => {
		const { session, requests, seen } = harness()
		const first = session.startOrUpdate("old")
		const second = session.startOrUpdate("new")
		requests[1].resolve({ snapshot: snapshot("new-id", "new") })
		await second
		requests[0].resolve({ snapshot: snapshot("old-id", "old") })
		await first
		expect(seen).toEqual(["new-id:new"])
		expect(session.getState().snapshot?.search_id).toBe("new-id")
		expect(requests[2]).toMatchObject({
			method: "search/cancel",
			params: { searchId: "old-id" },
		})
	})
	test("cancel before start response cannot resurrect even an empty-query popup", async () => {
		const { session, requests, seen } = harness()
		const first = session.startOrUpdate("")
		await session.cancel()
		session.handleNotification("search/completed", snapshot("old-id", ""))
		requests[0].resolve({ snapshot: snapshot("old-id", "") })
		await first
		expect(session.getState()).toEqual({ snapshot: null, error: null })
		expect(seen).toEqual([])
		expect(requests[1]).toMatchObject({
			method: "search/cancel",
			params: { searchId: "old-id" },
		})
	})
	test("late update cannot overwrite a newer update or cancel its shared id", async () => {
		const { session, requests, seen } = harness()
		const started = session.startOrUpdate("first")
		requests[0].resolve({ snapshot: snapshot("id", "first") })
		await started
		const old = session.startOrUpdate("old")
		const latest = session.startOrUpdate("latest")
		requests[2].resolve({ snapshot: snapshot("id", "latest") })
		await latest
		requests[1].resolve({ snapshot: snapshot("id", "old") })
		await old
		expect(seen).toEqual(["id:first", "id:latest"])
		expect(requests).toHaveLength(3)
	})
	test("confirmed start keeps early completed files instead of initial RPC snapshot", async () => {
		const { session, requests, seen } = harness()
		const started = session.startOrUpdate("same")
		session.handleNotification("search/completed", snapshot("stale-id", "same"))
		session.handleNotification("search/completed", snapshot("new-id", "same"))
		expect(seen).toEqual([])
		requests[0].resolve({ snapshot: snapshot("new-id", "same", false) })
		const result = await started
		expect(result.file_search_complete).toBe(true)
		expect(seen).toEqual(["new-id:same"])
	})
	test("completed notification while update awaits is not regressed by response", async () => {
		const { session, requests } = harness()
		const start = session.startOrUpdate("old")
		requests[0].resolve({ snapshot: snapshot("id", "old") })
		await start
		const update = session.startOrUpdate("new")
		session.handleNotification("search/completed", snapshot("id", "new"))
		requests[1].resolve({ snapshot: snapshot("id", "new", false) })
		expect((await update).file_search_complete).toBe(true)
	})
})

test("failure before start response survives without an endless loading state", async () => {
	const { session, requests } = harness()
	const start = session.startOrUpdate("")
	session.handleNotification("search/failed", {
		search_id: "id",
		query: "",
		message: "walk failed",
	})
	requests[0].resolve({ snapshot: snapshot("id", "", false) })
	expect((await start).file_search_complete).toBe(true)
	expect(session.getState().error).toBe("walk failed")
})
test("failure before update response is published as terminal", async () => {
	const { session, requests } = harness()
	const start = session.startOrUpdate("old")
	requests[0].resolve({ snapshot: snapshot("id", "old") })
	await start
	const update = session.startOrUpdate("new")
	session.handleNotification("search/failed", {
		search_id: "id",
		query: "new",
		message: "walk failed",
	})
	requests[1].resolve({ snapshot: snapshot("id", "new", false) })
	expect((await update).file_search_complete).toBe(true)
	expect(session.getState().error).toBe("walk failed")
})
test("late progress after failure cannot revive the loading indicator", async () => {
	const { session, requests } = harness()
	const start = session.startOrUpdate("query")
	requests[0].resolve({ snapshot: snapshot("id", "query", false) })
	await start
	session.handleNotification("search/failed", {
		search_id: "id",
		query: "query",
		message: "failed",
	})
	session.handleNotification("search/updated", snapshot("id", "query", false))
	expect(session.getState().snapshot?.file_search_complete).toBe(true)
	expect(session.getState().error).toBe("failed")
})
