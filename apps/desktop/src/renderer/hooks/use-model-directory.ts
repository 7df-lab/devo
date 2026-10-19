import type { CredentialInfo, ModelInfo } from "@devo-ai/sdk/v2/client"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useAtomValue } from "jotai"
import { useCallback } from "react"
import { serverConnectedAtom } from "../atoms/connection"
import { isMockModeAtom } from "../atoms/mock-mode"
import { invalidateProviderDependentQueries } from "../lib/invalidate-provider-queries"
import { getBaseClient } from "../services/connection-manager"

export interface ModelDirectoryData {
	models: ModelInfo[]
	credentials: CredentialInfo[]
}

/** Shares the TUI's Native catalog sources; network refresh remains server-owned. */
export function useModelDirectory() {
	const connected = useAtomValue(serverConnectedAtom)
	const mockMode = useAtomValue(isMockModeAtom)
	const queryClient = useQueryClient()
	const query = useQuery({
		queryKey: ["modelDirectory"],
		enabled: connected && !mockMode,
		queryFn: async (): Promise<ModelDirectoryData> => {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")
			const [models, credentials] = await Promise.all([
				client.model.list(),
				client.credential.list(),
			])
			return {
				models: models.data.models,
				credentials: credentials.data.credentials,
			}
		},
	})
	const refresh = useMutation({
		mutationFn: async (policy: "ifStale" | "force") => {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")
			const { data } = await client.model.refreshCatalog({ policy })
			if (data.status === "failed") throw new Error(data.message ?? "Remote catalog refresh failed")
			return data
		},
		onSuccess: () => {
			invalidateProviderDependentQueries()
			void queryClient.invalidateQueries({ queryKey: ["modelDirectory"] })
		},
	})
	const refreshCatalog = useCallback(
		(policy: "ifStale" | "force") => {
			if (connected && !mockMode) refresh.mutate(policy)
		},
		[connected, mockMode, refresh.mutate],
	)
	return {
		data: query.data ?? null,
		loading: query.isLoading,
		refreshing: refresh.isPending,
		offline: refresh.data?.status === "offline",
		error: refresh.error?.message ?? query.error?.message ?? null,
		refreshCatalog,
	}
}
