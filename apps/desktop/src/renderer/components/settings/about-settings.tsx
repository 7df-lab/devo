import { Button } from "@devo/ui/components/button"
import { CheckCircle2Icon, DownloadIcon, Loader2Icon } from "lucide-react"
import { useCallback, useEffect, useState } from "react"
import { useUpdater } from "../../hooks/use-updater"
import { SettingsRow } from "./settings-row"
import { SettingsSection } from "./settings-section"

const isElectron = typeof window !== "undefined" && "devo" in window

export function AboutSettings() {
	const [appVersion, setAppVersion] = useState("")
	const [isDev, setIsDev] = useState(false)
	const [cliInstalled, setCliInstalled] = useState<boolean | null>(null)
	const [cliLoading, setCliLoading] = useState(false)
	const [cliError, setCliError] = useState<string | null>(null)

	const updater = useUpdater()

	useEffect(() => {
		if (!isElectron) return
		window.devo.getAppInfo().then((info) => {
			setAppVersion(info.version)
			setIsDev(info.isDev)
		})
		window.devo.cli.isInstalled().then(setCliInstalled)
	}, [])

	const handleCliInstall = useCallback(async () => {
		if (!isElectron) return
		setCliLoading(true)
		setCliError(null)
		try {
			const result = await window.devo.cli.install()
			if (result.success) {
				setCliInstalled(true)
			} else {
				setCliError(result.error ?? "Failed to install CLI")
			}
		} finally {
			setCliLoading(false)
		}
	}, [])

	const handleCliUninstall = useCallback(async () => {
		if (!isElectron) return
		setCliLoading(true)
		setCliError(null)
		try {
			const result = await window.devo.cli.uninstall()
			if (result.success) {
				setCliInstalled(false)
			} else {
				setCliError(result.error ?? "Failed to uninstall CLI")
			}
		} finally {
			setCliLoading(false)
		}
	}, [])

	return (
		<div className="space-y-8">
			<div>
				<h2 className="text-xl font-semibold">About</h2>
			</div>

			<SettingsSection>
				<SettingsRow label="Version" description={isDev ? "Development build" : undefined}>
					<span className="text-sm text-muted-foreground">{appVersion || "..."}</span>
				</SettingsRow>
				<SettingsRow
					label="Updates"
					description={
						updater.status === "available"
							? `Version ${updater.version} available`
							: updater.status === "ready"
								? "Update downloaded, restart to apply"
								: updater.status === "error"
									? (updater.error ?? "Update check failed")
									: undefined
					}
				>
					{updater.status === "idle" && (
						<Button variant="outline" size="sm" onClick={updater.checkForUpdates}>
							Check for updates
						</Button>
					)}
					{updater.status === "checking" && (
						<div className="flex items-center gap-2 text-sm text-muted-foreground">
							<Loader2Icon aria-hidden="true" className="size-4 animate-spin" />
							Checking...
						</div>
					)}
					{updater.status === "available" && (
						<Button variant="outline" size="sm" onClick={updater.downloadUpdate}>
							<DownloadIcon aria-hidden="true" className="size-4" />
							Download
						</Button>
					)}
					{updater.status === "downloading" && (
						<div className="flex items-center gap-2 text-sm text-muted-foreground">
							<Loader2Icon aria-hidden="true" className="size-4 animate-spin" />
							{updater.progress ? `${Math.round(updater.progress.percent)}%` : "Downloading..."}
						</div>
					)}
					{updater.status === "ready" && (
						<Button variant="outline" size="sm" onClick={updater.installUpdate}>
							Restart to update
						</Button>
					)}
					{updater.status === "error" && (
						<Button variant="outline" size="sm" onClick={updater.checkForUpdates}>
							Retry
						</Button>
					)}
				</SettingsRow>
			</SettingsSection>

			<SettingsSection title="CLI">
				<SettingsRow
					label="devo CLI"
					description={
						cliError
							? cliError
							: cliInstalled
								? "Installed at /usr/local/bin/devo"
								: "Install the devo command-line tool"
					}
				>
					{cliLoading ? (
						<Loader2Icon aria-hidden="true" className="size-4 animate-spin text-muted-foreground" />
					) : cliInstalled ? (
						<div className="flex items-center gap-2">
							<CheckCircle2Icon aria-hidden="true" className="size-4 text-green-500" />
							<Button variant="outline" size="sm" onClick={handleCliUninstall}>
								Uninstall
							</Button>
						</div>
					) : cliInstalled === false ? (
						<Button variant="outline" size="sm" onClick={handleCliInstall}>
							Install
						</Button>
					) : null}
				</SettingsRow>
			</SettingsSection>
		</div>
	)
}
