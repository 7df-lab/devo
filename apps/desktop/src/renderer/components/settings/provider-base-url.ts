/** Accept explicit HTTP(S) endpoints, including keyless loopback servers. */
export function isValidProviderBaseUrl(value: string): boolean {
	const endpoint = value.trim()
	if (!/^https?:\/\//i.test(endpoint)) return false
	try {
		const url = new URL(endpoint)
		return (url.protocol === "http:" || url.protocol === "https:") && !!url.hostname
	} catch {
		return false
	}
}
