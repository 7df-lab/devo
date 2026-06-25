/**
 * Settings tab for the local Devo stdio runtime.
 */

import { Button } from "@devo/ui/components/button"
import { RefreshCwIcon, SquareIcon, TerminalIcon } from "lucide-react"
import { useState } from "react"
import { useAtomValue } from "jotai"
import { serverConnectedAtom, serverUrlAtom } from "../../atoms/connection"
import { SettingsRow } from "./settings-row"
import { SettingsSection } from "./settings-section"

const isElectron = typeof window !== "undefined" && "devo" in window

export function ServerSettings() {
	const connected = useAtomValue(serverConnectedAtom)
	const url = useAtomValue(serverUrlAtom)
	const [restarting, setRestarting] = useState(false)
	const [stopping, setStopping] = useState(false)

	async function restart() {
		if (!isElectron) return
		setRestarting(true)
		try {
			await window.devo.restartDevo()
		} finally {
			setRestarting(false)
		}
	}

	async function stop() {
		if (!isElectron) return
		setStopping(true)
		try {
			await window.devo.stopDevo()
		} finally {
			setStopping(false)
		}
	}

	return (
		<div className="space-y-8">
			<div>
				<h2 className="text-xl font-semibold">Server</h2>
				<p className="mt-1 text-sm text-muted-foreground">
					Devo Desktop manages a private local stdio ACP process.
				</p>
			</div>

			<SettingsSection>
				<SettingsRow
					label="Local runtime"
					description={url ?? "stdio://local"}
				>
					<div className="flex items-center gap-2 text-sm">
						<span
							className={`size-2 rounded-full ${connected ? "bg-emerald-500" : "bg-muted-foreground"}`}
						/>
						<span className="text-muted-foreground">{connected ? "Connected" : "Offline"}</span>
					</div>
				</SettingsRow>
				<SettingsRow
					label="Transport"
					description="ACP over child-process stdin/stdout"
				>
					<TerminalIcon aria-hidden="true" className="size-4 text-muted-foreground" />
				</SettingsRow>
			</SettingsSection>

			<SettingsSection>
				<SettingsRow
					label="Restart runtime"
					description="Stop the current child process and start a fresh Devo stdio server"
				>
					<Button size="sm" variant="outline" onClick={restart} disabled={restarting}>
						<RefreshCwIcon
							aria-hidden="true"
							className={`size-3.5 ${restarting ? "animate-spin" : ""}`}
						/>
						Restart
					</Button>
				</SettingsRow>
				<SettingsRow
					label="Stop runtime"
					description="Stop the managed Devo child process"
				>
					<Button size="sm" variant="outline" onClick={stop} disabled={stopping}>
						<SquareIcon aria-hidden="true" className="size-3.5" />
						Stop
					</Button>
				</SettingsRow>
			</SettingsSection>
		</div>
	)
}
