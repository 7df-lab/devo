/**
 * Full-page onboarding overlay.
 *
 * Renders a multi-step first-run experience that gates the main app.
 * Uses Framer Motion for step transitions and a progress indicator at the top.
 *
 * Core flow: Welcome -> Environment Check -> Complete (3 steps).
 * Migration from any detected provider (Claude Code, Cursor, Devo) is an
 * optional detour the user can trigger from the Complete screen.
 */

import { AnimatePresence, motion } from "motion/react"
import { useCallback, useState } from "react"
import type { MigrationPreview, MigrationProvider, MigrationResult } from "../../../preload/api"
import { APP_BAR_HEIGHT } from "../app-bar"
import { OnboardingProgress } from "./onboarding-progress"
import { CompleteStep } from "./steps/complete-step"
import { EnvironmentCheckStep } from "./steps/environment-check-step"
import { MigrationOfferStep } from "./steps/migration-offer-step"
import { MigrationPreviewStep } from "./steps/migration-preview-step"
import { ProviderSetupStep } from "./steps/provider-setup-step"
import { WelcomeStep } from "./steps/welcome-step"

// ============================================================
// Types
// ============================================================

export type OnboardingStep =
	| "welcome"
	| "environment"
	| "providers"
	| "complete"
	| "migration-offer"
	| "migration-preview"

interface OnboardingOverlayProps {
	onComplete: (state: {
		skippedSteps: string[]
		migrationPerformed: boolean
		migratedFrom: string[]
		devoVersion: string | null
		providersConnected: number
	}) => void
}

// ============================================================
// Constants
// ============================================================

/** Core steps shown in the progress indicator. Migration steps are a detour. */
const CORE_STEPS: OnboardingStep[] = ["welcome", "environment", "providers", "complete"]

const STEP_TRANSITION = {
	initial: { opacity: 0, y: 16 },
	animate: { opacity: 1, y: 0 },
	exit: { opacity: 0, y: -16 },
	transition: { duration: 0.25, ease: "easeOut" as const },
}

// ============================================================
// Component
// ============================================================

export function OnboardingOverlay({ onComplete }: OnboardingOverlayProps) {
	const [currentStep, setCurrentStep] = useState<OnboardingStep>("welcome")
	const [skippedSteps, setSkippedSteps] = useState<string[]>([])
	const [devoVersion, setDevoVersion] = useState<string | null>(null)
	const [providersConnected, setProvidersConnected] = useState(0)
	const [migratedProviders, setMigratedProviders] = useState<string[]>([])

	// Migration state (only populated if user opts in from complete screen)
	const [activeProvider, setActiveProvider] = useState<MigrationProvider | null>(null)
	const [scanResult, setScanResult] = useState<unknown>(null)
	const [selectedCategories, setSelectedCategories] = useState<string[]>([])
	const [migrationPreview, setMigrationPreview] = useState<MigrationPreview | null>(null)
	const [migrationResult, setMigrationResult] = useState<MigrationResult | null>(null)

	// For progress indicator, only show core steps
	const coreStepIndex = CORE_STEPS.indexOf(currentStep)
	// Migration steps show the same progress as "complete" (last dot)
	const displayIndex = coreStepIndex >= 0 ? coreStepIndex : CORE_STEPS.length - 1

	const goToStep = useCallback((step: OnboardingStep) => {
		setCurrentStep(step)
	}, [])

	const skipStep = useCallback((stepId: string) => {
		setSkippedSteps((prev) => [...prev, stepId])
	}, [])

	// --- Step handlers ---

	const handleWelcomeContinue = useCallback(() => {
		goToStep("environment")
	}, [goToStep])

	const handleEnvironmentComplete = useCallback(
		(version: string | null) => {
			setDevoVersion(version)
			goToStep("providers")
		},
		[goToStep],
	)

	const handleEnvironmentSkip = useCallback(() => {
		skipStep("environment")
		goToStep("providers")
	}, [goToStep, skipStep])

	const handleProvidersComplete = useCallback(
		(count: number) => {
			setProvidersConnected(count)
			goToStep("complete")
		},
		[goToStep],
	)

	const handleProvidersSkip = useCallback(() => {
		skipStep("providers")
		goToStep("complete")
	}, [goToStep, skipStep])

	// Migration opt-in from complete screen (now with provider selection)
	const handleStartMigration = useCallback(
		(provider: MigrationProvider) => {
			setActiveProvider(provider)
			// Reset migration state for this new provider
			setScanResult(null)
			setSelectedCategories([])
			setMigrationPreview(null)
			goToStep("migration-offer")
		},
		[goToStep],
	)

	const handleMigrationOfferPreview = useCallback(
		(scan: unknown, categories: string[], preview: MigrationPreview) => {
			setScanResult(scan)
			setSelectedCategories(categories)
			setMigrationPreview(preview)
			goToStep("migration-preview")
		},
		[goToStep],
	)

	const handleMigrationOfferSkip = useCallback(() => {
		setActiveProvider(null)
		goToStep("complete")
	}, [goToStep])

	const handleMigrationComplete = useCallback(
		(result: MigrationResult) => {
			setMigrationResult(result)
			if (activeProvider) {
				setMigratedProviders((prev) =>
					prev.includes(activeProvider) ? prev : [...prev, activeProvider],
				)
			}
			setActiveProvider(null)
			goToStep("complete")
		},
		[goToStep, activeProvider],
	)

	const handleMigrationBack = useCallback(() => {
		goToStep("migration-offer")
	}, [goToStep])

	const handleMigrationSkip = useCallback(() => {
		setActiveProvider(null)
		goToStep("complete")
	}, [goToStep])

	const handleFinish = useCallback(() => {
		onComplete({
			skippedSteps,
			migrationPerformed: migratedProviders.length > 0,
			migratedFrom: migratedProviders,
			devoVersion,
			providersConnected,
		})
	}, [onComplete, skippedSteps, migratedProviders, devoVersion, providersConnected])

	return (
		<div
			data-slot="onboarding-overlay"
			className="fixed inset-0 z-50 flex flex-col bg-background text-foreground"
		>
			{/* Reserve space for traffic lights / app bar area */}
			<div
				className="shrink-0"
				style={{
					height: APP_BAR_HEIGHT,
					// @ts-expect-error -- vendor-prefixed CSS property
					WebkitAppRegion: "drag",
				}}
			/>

			{/* Progress indicator (core steps only) */}
			<div className="shrink-0 px-8 py-2">
				<OnboardingProgress
					steps={CORE_STEPS}
					currentStep={currentStep}
					currentIndex={displayIndex}
					total={CORE_STEPS.length}
				/>
			</div>

			{/* Step content with transitions */}
			<div className="relative min-h-0 flex-1 overflow-hidden">
				<AnimatePresence mode="wait">
					{currentStep === "welcome" && (
						<motion.div
							key="welcome"
							className="absolute inset-0 overflow-y-auto"
							{...STEP_TRANSITION}
						>
							<WelcomeStep onContinue={handleWelcomeContinue} />
						</motion.div>
					)}

					{currentStep === "environment" && (
						<motion.div
							key="environment"
							className="absolute inset-0 overflow-y-auto"
							{...STEP_TRANSITION}
						>
							<EnvironmentCheckStep
								onComplete={handleEnvironmentComplete}
								onSkip={handleEnvironmentSkip}
							/>
						</motion.div>
					)}

					{currentStep === "providers" && (
						<motion.div
							key="providers"
							className="absolute inset-0 overflow-y-auto"
							{...STEP_TRANSITION}
						>
							<ProviderSetupStep
								onComplete={handleProvidersComplete}
								onSkip={handleProvidersSkip}
							/>
						</motion.div>
					)}

					{currentStep === "complete" && (
						<motion.div
							key="complete"
							className="absolute inset-0 overflow-y-auto"
							{...STEP_TRANSITION}
						>
							<CompleteStep
								devoVersion={devoVersion}
								migratedProviders={migratedProviders}
								migrationResult={migrationResult}
								onStartMigration={handleStartMigration}
								onFinish={handleFinish}
							/>
						</motion.div>
					)}

					{currentStep === "migration-offer" && activeProvider && (
						<motion.div
							key={`migration-offer-${activeProvider}`}
							className="absolute inset-0 overflow-y-auto"
							{...STEP_TRANSITION}
						>
							<MigrationOfferStep
								provider={activeProvider}
								onPreview={handleMigrationOfferPreview}
								onSkip={handleMigrationOfferSkip}
							/>
						</motion.div>
					)}

					{currentStep === "migration-preview" && activeProvider && (
						<motion.div
							key={`migration-preview-${activeProvider}`}
							className="absolute inset-0 overflow-y-auto"
							{...STEP_TRANSITION}
						>
							<MigrationPreviewStep
								provider={activeProvider}
								scanResult={scanResult}
								categories={selectedCategories}
								preview={migrationPreview}
								onComplete={handleMigrationComplete}
								onBack={handleMigrationBack}
								onSkip={handleMigrationSkip}
							/>
						</motion.div>
					)}
				</AnimatePresence>
			</div>
		</div>
	)
}
