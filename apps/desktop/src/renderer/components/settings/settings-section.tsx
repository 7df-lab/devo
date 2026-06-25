import { type ReactNode, useId } from "react"

interface SettingsSectionProps {
	title?: string
	description?: string
	children: ReactNode
}

export function SettingsSection({ title, description, children }: SettingsSectionProps) {
	const sectionId = useId()

	return (
		<section className="space-y-3" aria-labelledby={title ? sectionId : undefined}>
			{title && (
				<div>
					<h3 id={sectionId} className="text-sm font-medium">
						{title}
					</h3>
					{description && <p className="text-sm text-muted-foreground">{description}</p>}
				</div>
			)}
			<div className="divide-y divide-border rounded-lg border border-border">{children}</div>
		</section>
	)
}
