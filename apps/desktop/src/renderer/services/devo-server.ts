const BASE_URL = "http://localhost:3100"

async function jsonFetch<T>(path: string, init?: RequestInit): Promise<T> {
	const response = await fetch(`${BASE_URL}${path}`, init)
	if (!response.ok) {
		throw new Error(`Backend request failed: ${response.status} ${response.statusText}`)
	}
	return response.json() as Promise<T>
}

export async function fetchServers() {
	return jsonFetch<{ servers: unknown[] }>("/api/servers")
}

export async function fetchDevoUrl(): Promise<{ url: string }> {
	return jsonFetch<{ url: string }>("/api/servers/devo")
}

export async function fetchModelState(): Promise<{
	recent: { providerID: string; modelID: string }[]
	favorite: { providerID: string; modelID: string }[]
	variant: Record<string, string | undefined>
}> {
	return jsonFetch("/api/model-state")
}

export async function updateModelRecent(model: { providerID: string; modelID: string }): Promise<{
	recent: { providerID: string; modelID: string }[]
	favorite: { providerID: string; modelID: string }[]
	variant: Record<string, string | undefined>
}> {
	return jsonFetch("/api/model-state/recent", {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify(model),
	})
}

export async function checkServerHealth(): Promise<boolean> {
	try {
		const response = await fetch(`${BASE_URL}/health`)
		return response.ok
	} catch {
		return false
	}
}
