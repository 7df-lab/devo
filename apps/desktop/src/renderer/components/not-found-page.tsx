import { Button } from "@devo/ui/components/button"
import { useRouter } from "@tanstack/react-router"
import { SearchXIcon } from "lucide-react"

export function NotFoundPage() {
	const router = useRouter()

	return (
		<div className="flex h-full items-center justify-center p-6">
			<div className="w-full max-w-md space-y-6">
				{/* Icon */}
				<div className="flex justify-center">
					<div className="flex size-14 items-center justify-center rounded-full border border-border bg-muted/50">
						<SearchXIcon className="size-7 text-muted-foreground" />
					</div>
				</div>

				{/* Title + message */}
				<div className="text-center">
					<h1 className="text-lg font-semibold text-foreground">Page not found</h1>
					<p className="mt-2 text-sm leading-relaxed text-muted-foreground">
						The page you're looking for doesn't exist or has been moved.
					</p>
				</div>

				{/* Actions */}
				<div className="flex items-center justify-center">
					<Button variant="outline" size="sm" onClick={() => router.navigate({ to: "/" })}>
						Go home
					</Button>
				</div>
			</div>
		</div>
	)
}
