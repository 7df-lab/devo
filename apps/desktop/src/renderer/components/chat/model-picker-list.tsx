import {
	SearchableListPopoverEmpty,
	SearchableListPopoverList,
	SearchableListPopoverSearch,
	useSearchableListPopoverSearch,
} from "@devo/ui/components/searchable-list-popover"
import { CheckIcon, SearchIcon } from "lucide-react"
import { useEffect, useMemo, useRef, useState } from "react"
import { filterPickerModels, type PickerModel } from "./model-picker-data"

export function ModelPickerList({
	models,
	activeValue,
	loading,
	error,
	onRetry,
	onSelect,
	onManageProviders,
	refreshing = false,
	offline = false,
	onRefresh,
}: {
	models: PickerModel[]
	activeValue: string | null
	loading: boolean
	error: string | null
	onRetry: () => void
	onSelect: (model: PickerModel) => void
	onManageProviders: () => void
	refreshing?: boolean
	offline?: boolean
	onRefresh?: () => void
}) {
	const search = useSearchableListPopoverSearch()
	const [scope, setScope] = useState<"ready" | "all">("ready")
	const [limit, setLimit] = useState(60)
	const listRef = useRef<HTMLDivElement>(null)
	const filtered = useMemo(() => filterPickerModels(models, search, scope), [models, search, scope])
	const allMatches = useMemo(() => filterPickerModels(models, search, "all"), [models, search])
	useEffect(() => setLimit(60), [search, scope])

	return (
		<div
			onKeyDown={(event) => {
				if (event.key !== "ArrowDown" && event.key !== "ArrowUp" && event.key !== "Enter") return
				const rows = [
					...(listRef.current?.querySelectorAll<HTMLButtonElement>("button[data-model-option]") ??
						[]),
				]
				if (!rows.length) return
				const index = rows.indexOf(document.activeElement as HTMLButtonElement)
				if (event.key === "Enter") {
					if (event.target instanceof HTMLInputElement) {
						event.preventDefault()
						rows[0].click()
					}
					return
				}
				if (index === -1 && !(event.target instanceof HTMLInputElement)) return
				event.preventDefault()
				rows[
					index === -1
						? event.key === "ArrowDown"
							? 0
							: rows.length - 1
						: (index + (event.key === "ArrowDown" ? 1 : -1) + rows.length) % rows.length
				].focus()
			}}
		>
			<SearchableListPopoverSearch
				placeholder="Search models or providers…"
				icon={<SearchIcon className="size-3.5 stroke-[1.5]" />}
			/>
			<div className="flex gap-1 border-b p-2" role="group" aria-label="Model availability">
				{(["ready", "all"] as const).map((value) => (
					<button
						key={value}
						type="button"
						aria-pressed={scope === value}
						onClick={() => setScope(value)}
						className={`rounded-md px-2.5 py-1 text-xs transition-colors ${scope === value ? "bg-muted text-foreground" : "text-muted-foreground hover:bg-muted/60"}`}
					>
						{value === "ready" ? "Ready to use" : "All models"}
					</button>
				))}
			</div>
			<div ref={listRef}>
				<SearchableListPopoverList maxHeight="max-h-72">
					{filtered.slice(0, limit).map((model) => (
						<button
							key={model.value}
							type="button"
							data-model-option
							aria-label={`${model.displayName}, ${model.providerName}${model.ready ? "" : ", connect provider"}`}
							aria-current={model.value === activeValue ? "true" : undefined}
							onClick={() => onSelect(model)}
							className="flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-[13px] hover:bg-muted focus-visible:bg-muted focus-visible:outline-none"
						>
							<span
								className="flex size-3.5 shrink-0 items-center justify-center"
								aria-hidden="true"
							>
								{model.value === activeValue && <CheckIcon className="size-3.5 stroke-[1.5]" />}
							</span>
							<span className="min-w-0 flex-1">
								<span className="block truncate">{model.displayName}</span>
								<span
									className="block truncate text-[11px] text-muted-foreground"
									title={`${model.providerName} · ${model.modelID}`}
								>
									{model.providerName} · {model.modelID}
								</span>
							</span>
							{!model.ready && (
								<span className="shrink-0 text-xs text-muted-foreground">Connect</span>
							)}
						</button>
					))}
					{!filtered.length && (
						<SearchableListPopoverEmpty>
							{loading
								? "Loading models…"
								: search
									? "No matching models"
									: "Connect a provider to get started"}
							{scope === "ready" && allMatches.length > 0 && (
								<button
									type="button"
									className="mt-2 block w-full text-foreground underline underline-offset-4"
									onClick={() => setScope("all")}
								>
									Browse all models
								</button>
							)}
						</SearchableListPopoverEmpty>
					)}
					{filtered.length > limit && (
						<button
							type="button"
							className="w-full px-3 py-2 text-xs text-muted-foreground hover:bg-muted"
							onClick={() => setLimit((value) => value + 60)}
						>
							Show more · {filtered.length - limit} remaining
						</button>
					)}
				</SearchableListPopoverList>
			</div>
			{error && (
				<div role="status" className="border-t px-3 py-2 text-xs text-destructive">
					Could not update models. Your saved catalog is available.{" "}
					<button type="button" onClick={onRetry} className="underline">
						Retry
					</button>
				</div>
			)}
			<div className="flex items-center justify-between border-t px-3 py-2 text-xs text-muted-foreground">
				<span>
					{filtered.length} {filtered.length === 1 ? "model" : "models"}
				</span>
				{onRefresh && (
					<button
						type="button"
						onClick={onRefresh}
						disabled={refreshing}
						className="hover:text-foreground disabled:opacity-50"
						title={offline ? "Offline mode is enabled" : "Update catalog from remote"}
					>
						{refreshing ? "Updating…" : offline ? "Offline" : "Refresh"}
					</button>
				)}
				<button type="button" onClick={onManageProviders} className="hover:text-foreground">
					Manage providers
				</button>
			</div>
		</div>
	)
}
