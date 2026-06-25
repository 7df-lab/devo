import {
	PromptInputHeader,
	usePromptInputAttachments,
} from "@devo/ui/components/ai-elements/prompt-input"
import { AlertTriangleIcon, FileIcon, XIcon } from "lucide-react"
import { memo, useMemo } from "react"

interface PromptAttachmentPreviewProps {
	/** Whether the selected model supports image input */
	supportsImages?: boolean
	/** Whether the selected model supports PDF input */
	supportsPdf?: boolean
}

/**
 * Renders attachment previews inside the PromptInput header area.
 * Shows thumbnails for attached files with remove buttons.
 * Only renders when there are attachments.
 */
export const PromptAttachmentPreview = memo(function PromptAttachmentPreview({
	supportsImages,
	supportsPdf,
}: PromptAttachmentPreviewProps) {
	const { files, remove } = usePromptInputAttachments()

	// Check if any files are unsupported by the current model
	const warning = useMemo(() => {
		if (files.length === 0) return null
		const hasImages = files.some((f) => f.mediaType?.startsWith("image/"))
		const hasPdf = files.some((f) => f.mediaType === "application/pdf")
		const unsupported: string[] = []
		if (hasImages && supportsImages === false) unsupported.push("images")
		if (hasPdf && supportsPdf === false) unsupported.push("PDFs")
		if (unsupported.length === 0) return null
		return `Selected model may not support ${unsupported.join(" or ")}`
	}, [files, supportsImages, supportsPdf])

	if (files.length === 0) return null

	return (
		<PromptInputHeader>
			<div className="space-y-1.5 p-2">
				<div className="flex flex-wrap gap-2">
					{files.map((file) => {
						const isImage = file.mediaType?.startsWith("image/")
						return (
							<div
								key={file.id}
								className="group/preview relative size-16 shrink-0 overflow-hidden rounded-lg border border-border bg-muted"
							>
								{isImage ? (
									<img
										src={file.url}
										alt={file.filename ?? "Attachment"}
										className="size-full object-cover"
									/>
								) : (
									<div className="flex size-full flex-col items-center justify-center gap-0.5">
										<FileIcon className="size-5 text-muted-foreground" />
										<span className="max-w-[56px] truncate px-0.5 text-[8px] text-muted-foreground">
											{file.filename ?? "File"}
										</span>
									</div>
								)}
								<button
									type="button"
									onClick={() => remove(file.id)}
									className="absolute right-0.5 top-0.5 flex size-4 items-center justify-center rounded-full bg-black/60 text-white opacity-0 transition-opacity hover:bg-black/80 group-hover/preview:opacity-100"
								>
									<XIcon className="size-2.5" />
								</button>
							</div>
						)
					})}
				</div>
				{warning && (
					<div className="flex items-center gap-1.5 text-[11px] text-yellow-500">
						<AlertTriangleIcon className="size-3 shrink-0" />
						<span>{warning}</span>
					</div>
				)}
			</div>
		</PromptInputHeader>
	)
})
