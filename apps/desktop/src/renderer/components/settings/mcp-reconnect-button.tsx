import { Button } from "@devo/ui/components/button";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { RefreshCwIcon } from "lucide-react";
import { useRef } from "react";

interface McpReconnectClient {
	mcp: { reconnect: (params: { name: string }) => Promise<unknown> };
}

/** Reconnect only the existing configured transport, without changing credentials. */
export function McpReconnectButton({
	client,
	connected,
	directory,
	server,
}: {
	client: McpReconnectClient | null;
	connected: boolean;
	directory: string | null;
	server: { name: string; status: string };
}) {
	const queryClient = useQueryClient();
	// The ref also blocks repeated clicks before React renders the pending state.
	const inFlight = useRef(false);
	const reconnect = useMutation({
		mutationFn: async () => {
			if (!client || !connected) throw new Error("Not connected");
			await client.mcp.reconnect({ name: server.name });
		},
		retry: false,
		onSettled: async () => {
			try {
				await Promise.all([
					queryClient.invalidateQueries({
						queryKey: ["mcp-servers", directory],
					}),
					queryClient.invalidateQueries({
						queryKey: ["mcp-tools", directory, server.name],
					}),
				]);
			} finally {
				inFlight.current = false;
			}
		},
	});
	const disabled =
		!client ||
		!connected ||
		server.status === "disabled" ||
		server.status === "starting" ||
		reconnect.isPending;

	return (
		<div className="flex flex-col items-end gap-1">
			<Button
				type="button"
				size="sm"
				variant="ghost"
				disabled={disabled}
				aria-label={`Reconnect ${server.name}`}
				title="Restart the configured connection and refresh tools"
				onClick={() => {
					if (disabled || inFlight.current) return;
					inFlight.current = true;
					reconnect.mutate();
				}}
			>
				<RefreshCwIcon
					className={`size-3.5 stroke-[1.5]${reconnect.isPending ? " animate-spin" : ""}`}
				/>
				{reconnect.isPending ? "Reconnecting…" : "Reconnect"}
			</Button>
			{reconnect.error && (
				<p role="alert" className="max-w-64 text-xs text-destructive">
					Reconnect failed: {reconnect.error.message}. Try again.
				</p>
			)}
		</div>
	);
}
