import { PromptInputSubmit, usePromptInputController } from "@devo/ui/components/ai-elements/prompt-input"

export function ComposerSubmitControls({
	isWorking,
	isConnected,
	canSend,
	onStop,
}: {
	isWorking: boolean
	isConnected: boolean
	canSend: boolean
	onStop: () => void
}) {
	const { textInput } = usePromptInputController()
	const hasDraft = textInput.value.trim().length > 0

	return (
		<>
			{isWorking && (
				<PromptInputSubmit
					disabled={!isConnected}
					variant={hasDraft ? "ghost" : "default"}
					status="streaming"
					onStop={onStop}
				/>
			)}
			{(!isWorking || hasDraft) && (
				<PromptInputSubmit
					aria-label={isWorking ? "Queue message" : "Submit"}
					title={isWorking ? "Queue message" : "Send message"}
					disabled={!canSend}
				/>
			)}
		</>
	)
}
