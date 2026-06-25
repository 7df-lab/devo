/**
 * Simple avatar for AI providers.
 * Displays the first letter of the provider name with a deterministic background color.
 */

// ============================================================
// Color palette — deterministic based on provider ID
// ============================================================

const AVATAR_COLORS = [
	"bg-blue-500/20 text-blue-400",
	"bg-purple-500/20 text-purple-400",
	"bg-green-500/20 text-green-400",
	"bg-amber-500/20 text-amber-400",
	"bg-rose-500/20 text-rose-400",
	"bg-cyan-500/20 text-cyan-400",
	"bg-indigo-500/20 text-indigo-400",
	"bg-emerald-500/20 text-emerald-400",
	"bg-orange-500/20 text-orange-400",
	"bg-pink-500/20 text-pink-400",
]

function hashString(str: string): number {
	let hash = 0
	for (let i = 0; i < str.length; i++) {
		hash = (hash << 5) - hash + str.charCodeAt(i)
		hash |= 0
	}
	return Math.abs(hash)
}

// ============================================================
// Component
// ============================================================

interface ProviderAvatarProps {
	id: string
	name: string
	size?: "sm" | "md"
	className?: string
}

export function ProviderAvatar({ id, name, size = "md", className = "" }: ProviderAvatarProps) {
	const colorClass = AVATAR_COLORS[hashString(id) % AVATAR_COLORS.length]
	const letter = name.charAt(0).toUpperCase()
	const sizeClass = size === "sm" ? "size-7 text-xs" : "size-8 text-sm"

	return (
		<div
			className={`flex shrink-0 items-center justify-center rounded-md font-semibold ${sizeClass} ${colorClass} ${className}`}
			aria-hidden="true"
		>
			{letter}
		</div>
	)
}
