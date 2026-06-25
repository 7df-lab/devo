/**
 * Step progress indicator for the onboarding flow.
 *
 * Shows a row of dots: filled = completed, ring with fill = current, empty ring = upcoming.
 */

import type { OnboardingStep } from "./onboarding-overlay"

interface OnboardingProgressProps {
	steps: OnboardingStep[]
	currentStep: OnboardingStep
	currentIndex: number
	total: number
}

export function OnboardingProgress({
	steps,
	currentStep,
	currentIndex,
	total,
}: OnboardingProgressProps) {
	return (
		<div className="flex items-center justify-center gap-3">
			<div className="flex items-center gap-2">
				{steps.map((step, i) => {
					const isCompleted = i < currentIndex
					const isCurrent = step === currentStep

					return (
						<div
							key={step}
							className={`size-2 rounded-full transition-all duration-300 ${
								isCompleted
									? "bg-foreground"
									: isCurrent
										? "bg-foreground/60 ring-1 ring-foreground/30 ring-offset-1 ring-offset-transparent"
										: "bg-muted-foreground/20"
							}`}
						/>
					)
				})}
			</div>
			<span className="text-xs text-muted-foreground">
				{currentIndex + 1} of {total}
			</span>
		</div>
	)
}
