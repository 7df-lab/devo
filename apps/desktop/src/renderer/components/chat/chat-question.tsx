import { Button } from "@devo/ui/components/button"
import {
	ArrowRightIcon,
	Loader2Icon,
	MessageCircleQuestionIcon,
	SendIcon,
	SkipForwardIcon,
	ZapIcon,
} from "lucide-react"
import { memo, useCallback, useEffect, useRef, useState } from "react"
import type { QuestionAnswer, QuestionInfo, QuestionRequest } from "../../lib/types"

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface ChatQuestionFlowProps {
	/** All pending question requests for this agent */
	questions: QuestionRequest[]
	onReply: (requestId: string, answers: QuestionAnswer[]) => Promise<void>
	onReject: (requestId: string) => Promise<void>
	disabled?: boolean
	/** When true, the question originated from a sub-agent session */
	isFromSubAgent?: boolean
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Build the final answers array from selections + custom text per question. */
function buildAnswers(
	questions: QuestionInfo[],
	selections: Map<number, Set<string>>,
	customTexts: Map<number, string>,
): QuestionAnswer[] {
	return questions.map((_, idx) => {
		const custom = (customTexts.get(idx) ?? "").trim()
		if (custom) return [custom]
		return Array.from(selections.get(idx) ?? []).slice(0, 1)
	})
}

/** Check that a single question index has at least one answer selected or typed. */
function isQuestionAnswered(
	index: number,
	selections: Map<number, Set<string>>,
	customTexts: Map<number, string>,
): boolean {
	const selected = selections.get(index)
	const custom = (customTexts.get(index) ?? "").trim()
	return (selected && selected.size > 0) || custom.length > 0
}

// ---------------------------------------------------------------------------
// Sub-component: single question renderer
// ---------------------------------------------------------------------------

interface QuestionSectionProps {
	info: QuestionInfo
	index: number
	selected: Set<string>
	customText: string
	onToggle: (index: number, label: string) => void
	onCustomChange: (index: number, value: string) => void
	onSubmitCustom?: () => void
	disabled: boolean
}

function QuestionSection({
	info,
	index,
	selected,
	customText,
	onToggle,
	onCustomChange,
	onSubmitCustom,
	disabled,
}: QuestionSectionProps) {
	// Note: the protocol doesn't support multi-select yet; always false for now.
	const isMultiple = false
	const allowCustom = info.isOther !== false

	return (
		<fieldset aria-label={info.header} className="border-none p-0 m-0">
			<legend className="sr-only">{info.question}</legend>
			{/* Options */}
			<div
				role="radiogroup"
				aria-label={info.header}
				className="space-y-1.5 px-3 pt-3 pb-1.5"
			>
				{info.options.map((option: { label: string; description: string }) => {
					const isSelected = selected.has(option.label)

					return (
						<button
							key={option.label}
							type="button"
							role="radio"
							aria-checked={isSelected}
							onClick={() => onToggle(index, option.label)}
							disabled={disabled}
							className={`flex w-full items-start gap-2.5 rounded-lg border px-2.5 py-2 text-left text-sm transition-all ${
								isSelected
									? "border-primary/30 bg-primary/10 shadow-xs"
									: "border-transparent bg-muted/25 hover:border-border/70 hover:bg-muted/50"
							} ${disabled ? "cursor-not-allowed opacity-50" : "cursor-pointer"}`}
						>
							{/* Radio / checkbox indicator */}
							<span
								className={`mt-0.5 flex size-3.5 shrink-0 items-center justify-center border transition-colors ${
									isMultiple ? "rounded" : "rounded-full"
								} ${isSelected ? "border-primary bg-primary" : "border-muted-foreground/40 bg-card"}`}
								aria-hidden="true"
							>
								{isSelected && (
									<svg
										viewBox="0 0 12 12"
										className="size-2 fill-current text-primary-foreground"
										aria-hidden="true"
									>
										{isMultiple ? (
											<path
												d="M10 3L4.5 8.5L2 6"
												fill="none"
												stroke="currentColor"
												strokeWidth="2"
												strokeLinecap="round"
												strokeLinejoin="round"
											/>
										) : (
											<circle cx="6" cy="6" r="3" />
										)}
									</svg>
								)}
							</span>

							{/* Label + description */}
							<span className="min-w-0 flex-1">
								<span className="font-medium text-foreground">{option.label}</span>
								{option.description && (
									<span className="block text-muted-foreground text-xs mt-0.5 line-clamp-2">
										{option.description}
									</span>
								)}
							</span>
						</button>
					)
				})}
			</div>

			{/* Custom text input */}
			{allowCustom && (
				<div className="px-3 pb-3 pt-1.5">
					<label htmlFor={`question-custom-${index}`} className="sr-only">
						Other answer for {info.header || info.question}
					</label>
					<input
						id={`question-custom-${index}`}
						type={info.isSecret ? "password" : "text"}
						value={customText}
						onChange={(e) => onCustomChange(index, e.target.value)}
						onKeyDown={(e) => {
							if (e.key === "Enter" && !e.shiftKey) {
								e.preventDefault()
								onSubmitCustom?.()
							}
						}}
						placeholder={info.isSecret ? "Type a secret value..." : "Type a custom answer..."}
						disabled={disabled}
						className="w-full rounded-lg border border-border/70 bg-background px-3 py-2 text-sm text-foreground shadow-xs placeholder:text-muted-foreground/50 transition-[border-color,box-shadow] focus:border-primary/50 focus:outline-none focus:ring-2 focus:ring-primary/10 disabled:cursor-not-allowed disabled:opacity-50"
					/>
				</div>
			)}
		</fieldset>
	)
}

// ---------------------------------------------------------------------------
// Progress dots
// ---------------------------------------------------------------------------

function StepDots({
	total,
	current,
	answered,
}: {
	total: number
	current: number
	answered: Set<number>
}) {
	if (total <= 1) return null
	const dots = []
	for (let i = 0; i < total; i++) {
		dots.push(
			<span
				key={`dot-${i}-of-${total}`}
				className={`size-1.5 rounded-full transition-colors ${
					i === current
						? "bg-foreground"
						: answered.has(i)
							? "bg-foreground/40"
							: "bg-muted-foreground/25"
				}`}
				aria-hidden="true"
			/>,
		)
	}
	return (
		<span className="flex items-center gap-1" aria-hidden="true">
			{dots}
		</span>
	)
}

// ---------------------------------------------------------------------------
// Main component: question flow (replaces chat input entirely)
// ---------------------------------------------------------------------------

export const ChatQuestionFlow = memo(function ChatQuestionFlow({
	questions,
	onReply,
	onReject,
	disabled = false,
	isFromSubAgent = false,
}: ChatQuestionFlowProps) {
	// Current question request being worked on (first in the queue)
	const currentRequest = questions[0]
	if (!currentRequest) return null

	return (
		<QuestionRequestStepper
			key={currentRequest.id}
			request={currentRequest}
			totalRequests={questions.length}
			onReply={onReply}
			onReject={onReject}
			disabled={disabled}
			isFromSubAgent={isFromSubAgent}
		/>
	)
})

// ---------------------------------------------------------------------------
// Inner component: handles stepping through QuestionInfos in one request
// ---------------------------------------------------------------------------

interface QuestionRequestStepperProps {
	request: QuestionRequest
	totalRequests: number
	onReply: (requestId: string, answers: QuestionAnswer[]) => Promise<void>
	onReject: (requestId: string) => Promise<void>
	disabled: boolean
	isFromSubAgent?: boolean
}

const QuestionRequestStepper = memo(function QuestionRequestStepper({
	request,
	totalRequests,
	onReply,
	onReject,
	disabled,
	isFromSubAgent = false,
}: QuestionRequestStepperProps) {
	const questions = request.questions
	const totalSteps = questions.length

	const [currentStep, setCurrentStep] = useState(0)
	const [selections, setSelections] = useState<Map<number, Set<string>>>(() => new Map())
	const [customTexts, setCustomTexts] = useState<Map<number, string>>(() => new Map())
	const [submitting, setSubmitting] = useState(false)
	const cardRef = useRef<HTMLElement>(null)

	const currentQuestion = questions[currentStep]
	const isLastStep = currentStep === totalSteps - 1
	const currentAnswered = isQuestionAnswered(currentStep, selections, customTexts)

	// Track which steps have been answered
	const answeredSteps = new Set<number>()
	for (let i = 0; i < totalSteps; i++) {
		if (isQuestionAnswered(i, selections, customTexts)) {
			answeredSteps.add(i)
		}
	}

	// --- Selection toggle ---
	const handleToggle = useCallback(
		(questionIndex: number, label: string) => {
			setCustomTexts((prev) => {
				if (!prev.has(questionIndex)) return prev
				const next = new Map(prev)
				next.delete(questionIndex)
				return next
			})
			setSelections((prev) => {
				const next = new Map(prev)
				const current = new Set(next.get(questionIndex) ?? [])
				current.clear()
				current.add(label)
				next.set(questionIndex, current)
				return next
			})
		},
		[],
	)

	// --- Custom text change ---
	const handleCustomChange = useCallback((questionIndex: number, value: string) => {
		if (value.trim()) {
			setSelections((prev) => {
				if (!prev.has(questionIndex)) return prev
				const next = new Map(prev)
				next.delete(questionIndex)
				return next
			})
		}
		setCustomTexts((prev) => {
			const next = new Map(prev)
			next.set(questionIndex, value)
			return next
		})
	}, [])

	// --- Advance to next step or submit ---
	const handleNext = useCallback(() => {
		if (!currentAnswered || disabled || submitting) return
		if (!isLastStep) {
			setCurrentStep((s) => s + 1)
		}
	}, [currentAnswered, disabled, submitting, isLastStep])

	// --- Submit all answers ---
	const handleSubmit = useCallback(async () => {
		if (disabled || submitting) return
		// If on last step, current must be answered. Otherwise all must be answered.
		if (isLastStep && !currentAnswered) return
		setSubmitting(true)
		try {
			const answers = buildAnswers(questions, selections, customTexts)
			await onReply(request.id, answers)
		} finally {
			setSubmitting(false)
		}
	}, [
		disabled,
		submitting,
		isLastStep,
		currentAnswered,
		questions,
		selections,
		customTexts,
		onReply,
		request.id,
	])

	// Combined handler: next or submit
	const handleAdvance = useCallback(() => {
		if (isLastStep) {
			handleSubmit()
		} else {
			handleNext()
		}
	}, [isLastStep, handleSubmit, handleNext])

	// --- Skip entire request ---
	const handleSkip = useCallback(async () => {
		if (disabled || submitting) return
		setSubmitting(true)
		try {
			await onReject(request.id)
		} finally {
			setSubmitting(false)
		}
	}, [disabled, submitting, onReject, request.id])

	// --- Go back ---
	const handleBack = useCallback(() => {
		if (currentStep > 0) setCurrentStep((s) => s - 1)
	}, [currentStep])

	// --- Keyboard shortcuts ---
	useEffect(() => {
		function handleKeyDown(e: KeyboardEvent) {
			// Allow Enter from custom input to advance
			if (e.target instanceof HTMLInputElement && e.target.id?.startsWith("question-custom-")) {
				return // handled by onSubmitCustom prop
			}

			if (e.key === "Enter" && !e.shiftKey && currentAnswered) {
				e.preventDefault()
				handleAdvance()
			} else if (e.key === "Escape") {
				e.preventDefault()
				handleSkip()
			}
		}

		document.addEventListener("keydown", handleKeyDown)
		return () => document.removeEventListener("keydown", handleKeyDown)
	}, [currentAnswered, handleAdvance, handleSkip])

	// --- Auto-focus the card on mount and step change ---
	useEffect(() => {
		// Focus the custom input if available, otherwise the card
		const timer = requestAnimationFrame(() => {
			const customInput = document.getElementById(
				`question-custom-${currentStep}`,
			) as HTMLInputElement | null
			if (customInput) {
				customInput.focus()
			} else {
				cardRef.current?.focus()
			}
		})
		return () => cancelAnimationFrame(timer)
	}, [currentStep])

	if (!currentQuestion) return null

	return (
		<section
			ref={cardRef}
			tabIndex={-1}
			aria-label="Agent question"
			className="animate-in fade-in slide-in-from-bottom-2 overflow-hidden rounded-xl border border-border/70 bg-card shadow-lg shadow-black/5 ring-1 ring-foreground/5 outline-none duration-300"
		>
			{/* Sub-agent indicator */}
			{isFromSubAgent && (
				<div className="flex items-center gap-1 px-4 pt-3 text-[11px] font-medium text-muted-foreground/70">
					<ZapIcon className="size-3.5 shrink-0 stroke-[1.5]" aria-hidden="true" />
					<span>Sub-agent asking a question</span>
				</div>
			)}
			{/* Header */}
			<div className="flex items-start gap-3 px-4 py-3.5">
				<div className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary ring-1 ring-primary/15">
					<MessageCircleQuestionIcon
						className="size-3.5 stroke-[1.5]"
						aria-hidden="true"
					/>
				</div>
				<div className="min-w-0 flex-1">
					<div className="mb-0.5 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
						{currentQuestion.header || "Question"}
					</div>
					<div className="text-sm font-medium leading-5 text-foreground">
						{currentQuestion.question}
					</div>
				</div>
				{totalRequests > 1 && (
					<span className="mt-1 shrink-0 rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground">
						+{totalRequests - 1} more
					</span>
				)}
			</div>

			{/* Question content */}
			<div className="border-t border-border/60 bg-muted/10">
				<QuestionSection
					info={currentQuestion}
					index={currentStep}
					selected={selections.get(currentStep) ?? new Set()}
					customText={customTexts.get(currentStep) ?? ""}
					onToggle={handleToggle}
					onCustomChange={handleCustomChange}
					onSubmitCustom={currentAnswered ? handleAdvance : undefined}
					disabled={disabled || submitting}
				/>
			</div>

			{/* Footer with navigation */}
			<div className="flex items-center gap-2 border-t border-border/60 bg-card px-3 py-2.5">
				{/* Left side: back + step dots */}
				<div className="flex flex-1 items-center gap-2">
					{currentStep > 0 && (
						<button
							type="button"
							onClick={handleBack}
							disabled={disabled || submitting}
							className="text-xs text-muted-foreground transition-colors hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
						>
							Back
						</button>
					)}
					<StepDots total={totalSteps} current={currentStep} answered={answeredSteps} />
				</div>

				{/* Right side: skip + action button */}
				<button
					type="button"
					onClick={handleSkip}
					disabled={disabled || submitting}
					className="flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
					aria-label="Skip question"
				>
					<SkipForwardIcon className="size-3.5 stroke-[1.5]" aria-hidden="true" />
					Skip
				</button>
				{isLastStep ? (
					<Button
						size="sm"
						onClick={handleSubmit}
						disabled={!currentAnswered || disabled || submitting}
						className="h-7 gap-1 text-xs"
						aria-label="Send answer"
					>
						{submitting ? (
							<Loader2Icon className="size-3.5 animate-spin stroke-[1.5]" aria-hidden="true" />
						) : (
							<SendIcon className="size-3.5 stroke-[1.5]" aria-hidden="true" />
						)}
						Send
					</Button>
				) : (
					<Button
						size="sm"
						variant="secondary"
						onClick={handleNext}
						disabled={!currentAnswered || disabled || submitting}
						className="h-7 gap-1 text-xs"
						aria-label="Next question"
					>
						Next
						<ArrowRightIcon className="size-3.5 stroke-[1.5]" aria-hidden="true" />
					</Button>
				)}
			</div>
		</section>
	)
})
