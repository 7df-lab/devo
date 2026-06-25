/**
 * Create/edit automation dialog -- modal overlay for creating or editing an automation.
 *
 * In edit mode, shows metadata header (started date, run count, status),
 * pre-fills fields from the automation, and shows Delete/Test/Pause actions.
 */

import { Badge } from "@devo/ui/components/badge"
import { Button } from "@devo/ui/components/button"
import {
	Combobox,
	ComboboxContent,
	ComboboxEmpty,
	ComboboxInput,
	ComboboxItem,
	ComboboxList,
} from "@devo/ui/components/combobox"
import {
	Dialog,
	DialogContent,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import { Input } from "@devo/ui/components/input"
import { Label } from "@devo/ui/components/label"
import { Separator } from "@devo/ui/components/separator"
import { Textarea } from "@devo/ui/components/textarea"
import { useAtomValue } from "jotai"
import {
	FolderIcon,
	FolderOpenIcon,
	PauseIcon,
	PlayIcon,
	Trash2Icon,
	TriangleIcon,
} from "lucide-react"
import { useCallback, useEffect, useMemo, useState } from "react"
import { toast } from "sonner"
import type { Automation } from "../../../preload/api"
import { discoveryProjectsAtom } from "../../atoms/discovery"
import {
	useModelState,
	useDevoAgents,
	useProviders,
} from "../../hooks/use-devo-data"
import {
	createAutomation,
	deleteAutomation,
	pickDirectory,
	runAutomationNow,
	updateAutomation,
} from "../../services/backend"
import type { ModelRef } from "../../hooks/use-devo-data"
import { AgentSelector, ModelSelector, VariantSelector } from "../chat/prompt-toolbar"
import { SchedulePicker } from "./schedule-picker"
import { getModelVariants } from "../../hooks/use-devo-data"

interface CreateAutomationDialogProps {
	open: boolean
	onOpenChange: (open: boolean) => void
	/** When set, the dialog is in "edit" mode and pre-fills from this automation. */
	editAutomation?: Automation | null
}

const DEFAULT_RRULE = "FREQ=DAILY;BYHOUR=9;BYMINUTE=0"

// ============================================================
// Helpers
// ============================================================

function formatDate(ts: number): string {
	return new Date(ts).toLocaleDateString("en-US", {
		month: "short",
		day: "numeric",
		year: "numeric",
	})
}

function ProjectChip({ path, onRemove }: { path: string; onRemove: () => void }) {
	const name = path.split("/").pop() ?? path
	return (
		<span className="inline-flex items-center gap-1 rounded-md bg-accent px-2 py-0.5 text-xs">
			{name}
			<button
				type="button"
				onClick={onRemove}
				className="ml-0.5 text-muted-foreground hover:text-foreground"
				aria-label={`Remove ${name}`}
			>
				&times;
			</button>
		</span>
	)
}

// ============================================================
// Main component
// ============================================================

export function CreateAutomationDialog({
	open,
	onOpenChange,
	editAutomation,
}: CreateAutomationDialogProps) {
	const isEditing = !!editAutomation

	const [name, setName] = useState("")
	const [prompt, setPrompt] = useState("")
	const [workspaces, setWorkspaces] = useState<string[]>([])
	const [rrule, setRrule] = useState(DEFAULT_RRULE)
	const [isSubmitting, setIsSubmitting] = useState(false)
	const [isTesting, setIsTesting] = useState(false)

	// Model / agent / variant state
	const [selectedModel, setSelectedModel] = useState<ModelRef | null>(null)
	const [selectedAgent, setSelectedAgent] = useState<string | null>(null)
	const [selectedVariant, setSelectedVariant] = useState<string | undefined>(undefined)

	// Use the first workspace (or null) to scope provider/agent fetching.
	// Providers are server-level and consistent across directories; this is
	// the same pattern used by the new-chat screen.
	const directory = workspaces[0] ?? null

	const { data: providers } = useProviders(directory)
	const { agents } = useDevoAgents(directory)
	const { recentModels } = useModelState()

	// Compute available variants for the selected model
	const variants = useMemo(() => {
		if (!selectedModel || !providers) return []
		return getModelVariants(selectedModel.providerID, selectedModel.modelID, providers.providers)
	}, [selectedModel, providers])

	// Reset variant when model changes and the selected variant is no longer valid
	useEffect(() => {
		if (selectedVariant && !variants.includes(selectedVariant)) {
			setSelectedVariant(undefined)
		}
	}, [variants, selectedVariant])

	// Pre-fill form when editing
	useEffect(() => {
		if (open && editAutomation) {
			setName(editAutomation.name)
			setPrompt(editAutomation.prompt)
			setWorkspaces([...editAutomation.workspaces])
			setRrule(editAutomation.schedule.rrule)
			// Pre-fill model/agent/variant from execution config
			const exec = editAutomation.execution
			if (exec.model) {
				const slashIdx = exec.model.indexOf("/")
				if (slashIdx > 0) {
					setSelectedModel({
						providerID: exec.model.slice(0, slashIdx),
						modelID: exec.model.slice(slashIdx + 1),
					})
				}
			} else {
				setSelectedModel(null)
			}
			setSelectedAgent(exec.agent ?? null)
			setSelectedVariant(exec.variant ?? undefined)
		} else if (open && !editAutomation) {
			setName("")
			setPrompt("")
			setWorkspaces([])
			setRrule(DEFAULT_RRULE)
			setSelectedModel(null)
			setSelectedAgent(null)
			setSelectedVariant(undefined)
		}
	}, [open, editAutomation])

	const canSave = name.trim().length > 0 && prompt.trim().length > 0

	const buildExecutionPatch = useCallback(() => {
		const modelStr = selectedModel
			? `${selectedModel.providerID}/${selectedModel.modelID}`
			: undefined
		return {
			effort: "medium" as const,
			...(modelStr ? { model: modelStr } : {}),
			...(selectedAgent ? { agent: selectedAgent } : {}),
			...(selectedVariant ? { variant: selectedVariant } : {}),
		}
	}, [selectedModel, selectedAgent, selectedVariant])

	const handleSubmit = useCallback(async () => {
		if (!canSave || isSubmitting) return
		setIsSubmitting(true)
		try {
			if (isEditing && editAutomation) {
				await updateAutomation({
					id: editAutomation.id,
					name: name.trim(),
					prompt: prompt.trim(),
					schedule: {
						rrule,
						timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
					},
					workspaces,
					execution: buildExecutionPatch(),
				})
			} else {
				await createAutomation({
					name: name.trim(),
					prompt: prompt.trim(),
					schedule: {
						rrule,
						timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
					},
					workspaces,
					execution: buildExecutionPatch(),
				})
			}
			onOpenChange(false)
		} catch (err) {
			toast.error(isEditing ? "Failed to save automation" : "Failed to create automation", {
				description: err instanceof Error ? err.message : undefined,
			})
		} finally {
			setIsSubmitting(false)
		}
	}, [
		canSave,
		isSubmitting,
		isEditing,
		editAutomation,
		name,
		prompt,
		rrule,
		workspaces,
		onOpenChange,
		buildExecutionPatch,
	])

	// Discovered projects from Devo SDK
	const discoveredProjects = useAtomValue(discoveryProjectsAtom)
	const availableProjects = useMemo(
		() =>
			discoveredProjects
				.filter((p) => !workspaces.includes(p.worktree))
				.map((p) => ({
					value: p.worktree,
					label: p.name ?? p.worktree.split("/").pop() ?? p.worktree,
					path: p.worktree,
				})),
		[discoveredProjects, workspaces],
	)

	const handleCancel = useCallback(() => {
		onOpenChange(false)
	}, [onOpenChange])

	const handleAddProject = useCallback(async () => {
		const dir = await pickDirectory()
		if (dir && !workspaces.includes(dir)) {
			setWorkspaces((prev) => [...prev, dir])
		}
	}, [workspaces])

	const handleRemoveProject = useCallback((path: string) => {
		setWorkspaces((prev) => prev.filter((w) => w !== path))
	}, [])

	const handleDelete = useCallback(async () => {
		if (!editAutomation) return
		try {
			await deleteAutomation(editAutomation.id)
			onOpenChange(false)
		} catch (err) {
			toast.error("Failed to delete automation", {
				description: err instanceof Error ? err.message : undefined,
			})
		}
	}, [editAutomation, onOpenChange])

	const handleTest = useCallback(async () => {
		if (!editAutomation || isTesting) return
		setIsTesting(true)
		try {
			await runAutomationNow(editAutomation.id)
			toast.success("Automation run started", {
				description: "Check the inbox for results.",
			})
			onOpenChange(false)
		} catch (err) {
			toast.error("Failed to run automation", {
				description: err instanceof Error ? err.message : undefined,
			})
		} finally {
			setIsTesting(false)
		}
	}, [editAutomation, isTesting, onOpenChange])

	const handleTogglePause = useCallback(async () => {
		if (!editAutomation) return
		try {
			await updateAutomation({
				id: editAutomation.id,
				status: editAutomation.status === "paused" ? "active" : "paused",
			})
			onOpenChange(false)
		} catch (err) {
			toast.error("Failed to update automation", {
				description: err instanceof Error ? err.message : undefined,
			})
		}
	}, [editAutomation, onOpenChange])

	const isPaused = editAutomation?.status === "paused"

	// Default agent for the AgentSelector fallback display
	const defaultAgent = agents[0]?.name

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-lg">
				<DialogHeader>
					<div className="flex items-start justify-between gap-4">
						<DialogTitle>{isEditing ? "Edit automation" : "Create automation"}</DialogTitle>
						{isEditing && editAutomation && (
							<div className="flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
								<span>Started {formatDate(editAutomation.createdAt)}</span>
								<span>{editAutomation.runCount} runs</span>
								<Badge
									variant={isPaused ? "secondary" : "default"}
									className="gap-1 px-1.5 py-0 text-[10px]"
								>
									<span
										className={`inline-block size-1.5 rounded-full ${isPaused ? "bg-yellow-500" : "bg-green-500"}`}
									/>
									{isPaused ? "Paused" : "Active"}
								</Badge>
							</div>
						)}
					</div>
				</DialogHeader>

				<div className="space-y-5">
					{/* Name */}
					<div className="space-y-2">
						<Label htmlFor="automation-name">Name</Label>
						<Input
							id="automation-name"
							value={name}
							onChange={(e) => setName(e.target.value)}
							placeholder="Check for sentry issues"
							autoFocus
						/>
					</div>

					{/* Projects */}
					<div className="space-y-2">
						<Label>Projects</Label>
						<p className="text-xs text-muted-foreground">
							If you want an automation to run on a specific branch, you can specify it in your
							prompt.
						</p>

						{/* Selected projects as chips */}
						{workspaces.length > 0 && (
							<div className="flex flex-wrap items-center gap-1.5">
								{workspaces.map((w) => (
									<ProjectChip key={w} path={w} onRemove={() => handleRemoveProject(w)} />
								))}
							</div>
						)}

						{/* Project combobox -- select from discovered projects */}
						{availableProjects.length > 0 && (
							<Combobox
								value={null}
								onValueChange={(value) => {
									if (value && !workspaces.includes(value)) {
										setWorkspaces((prev) => [...prev, value])
									}
								}}
							>
								<ComboboxInput placeholder="Search projects..." showClear={false} />
								<ComboboxContent>
									<ComboboxList>
										{availableProjects.map((project) => (
											<ComboboxItem key={project.value} value={project.value}>
												<FolderIcon
													aria-hidden="true"
													className="size-3.5 shrink-0 text-muted-foreground"
												/>
												<div className="flex flex-col gap-0.5 overflow-hidden">
													<span className="truncate text-sm">{project.label}</span>
													{project.label !== project.path && (
														<span className="truncate text-xs text-muted-foreground">
															{project.path}
														</span>
													)}
												</div>
											</ComboboxItem>
										))}
										<ComboboxEmpty>No projects found</ComboboxEmpty>
									</ComboboxList>
								</ComboboxContent>
							</Combobox>
						)}

						<button
							type="button"
							onClick={handleAddProject}
							className="inline-flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground"
						>
							<FolderOpenIcon aria-hidden="true" className="size-3.5" />
							Browse for folder
						</button>

						{isEditing && (
							<p className="text-xs text-muted-foreground">
								Automations run in the background on dedicated worktrees. Automations in
								non-version-controlled projects run directly in the project directory.
							</p>
						)}
					</div>

					{/* Prompt */}
					<div className="space-y-2">
						<Label htmlFor="automation-prompt">Prompt</Label>
						<Textarea
							id="automation-prompt"
							value={prompt}
							onChange={(e) => setPrompt(e.target.value)}
							placeholder="look for crashes in $Sentry"
							className="min-h-[100px]"
						/>
					</div>

					{/* Schedule */}
					<SchedulePicker value={rrule} onChange={setRrule} />

					{/* Model / Agent / Variant */}
					<div className="space-y-2">
						<Label>Agent &amp; model</Label>
						<p className="text-xs text-muted-foreground">
							Choose which agent mode and model to use. Defaults to your server configuration.{" "}
							{!directory && "Add a project above to load available models."}
						</p>
						<div className="flex flex-wrap items-center gap-1 rounded-md border px-1 py-1">
							{agents.length > 0 && (
								<>
									<AgentSelector
										agents={agents}
										selectedAgent={selectedAgent}
										defaultAgent={defaultAgent}
										onSelectAgent={setSelectedAgent}
									/>
									<Separator orientation="vertical" className="mx-0.5 my-1 self-stretch" />
								</>
							)}
							<ModelSelector
								providers={providers}
								effectiveModel={selectedModel}
								hasOverride={selectedModel !== null}
								onSelectModel={setSelectedModel}
								recentModels={recentModels}
							/>
							{variants.length > 0 && (
								<>
									<Separator orientation="vertical" className="mx-0.5 my-1 self-stretch" />
									<VariantSelector
										variants={variants}
										selectedVariant={selectedVariant}
										onSelectVariant={setSelectedVariant}
									/>
								</>
							)}
						</div>
					</div>
				</div>

				<DialogFooter className="flex-row justify-between sm:justify-between">
					{/* Left side: destructive / action buttons (only in edit mode) */}
					<div className="flex items-center gap-2">
						{isEditing && (
							<>
								<Button
									variant="ghost"
									size="sm"
									className="gap-1.5 text-destructive hover:text-destructive"
									onClick={handleDelete}
								>
									<Trash2Icon className="size-3.5" />
									Delete
								</Button>
								<Button
									variant="ghost"
									size="sm"
									className="gap-1.5"
									onClick={handleTest}
									disabled={isTesting}
								>
									<TriangleIcon className="size-3 rotate-90 fill-current" />
									{isTesting ? "Running..." : "Test"}
								</Button>
								<Button variant="ghost" size="sm" className="gap-1.5" onClick={handleTogglePause}>
									{isPaused ? (
										<PlayIcon className="size-3.5" />
									) : (
										<PauseIcon className="size-3.5" />
									)}
									{isPaused ? "Resume" : "Pause"}
								</Button>
							</>
						)}
					</div>

					{/* Right side: cancel / save */}
					<div className="flex items-center gap-2">
						<Button variant="ghost" onClick={handleCancel}>
							Cancel
						</Button>
						<Button onClick={handleSubmit} disabled={!canSave || isSubmitting}>
							{isSubmitting
								? isEditing
									? "Saving..."
									: "Creating..."
								: isEditing
									? "Save"
									: "Create"}
						</Button>
					</div>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	)
}
