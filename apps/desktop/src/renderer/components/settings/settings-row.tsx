import type { ReactNode } from "react"

interface SettingsRowProps {
	label: string
	description?: string
	/** ID of the control to associate with this row label, when applicable. */
	htmlFor?: string
	children: ReactNode
}

export function SettingsRow({ label, description, htmlFor, children }: SettingsRowProps) {
	return (
		<div className="flex items-center justify-between gap-4 px-4 py-3">
			<div className="flex min-w-0 flex-col gap-0.5">
				{htmlFor ? (
					<label htmlFor={htmlFor} className="text-sm font-normal tracking-tight text-foreground">
						{label}
					</label>
				) : (
					<span className="text-sm font-normal tracking-tight text-foreground">{label}</span>
				)}
				{description && (
					<span className="text-xs leading-5 text-muted-foreground">{description}</span>
				)}
			</div>
			<div className="flex shrink-0 items-center gap-2">{children}</div>
		</div>
	)
}
