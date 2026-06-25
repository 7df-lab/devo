/**
 * Provider logo icons fetched from models.dev at runtime.
 * Uses `dark:invert` to adapt black-on-transparent SVGs to dark mode.
 *
 * Falls back to a colored letter avatar when the logo fails to load.
 */

import { useState } from "react"

// ============================================================
// Color palette for letter avatars (fallback)
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

const SIZE_CLASSES = {
	xs: "size-4",
	sm: "size-7",
	md: "size-8",
	lg: "size-10",
} as const

/** Pixel sizes matching the Tailwind size classes, used for img width/height attributes */
const SIZE_PX = {
	xs: 16,
	sm: 28,
	md: 32,
	lg: 40,
} as const

interface ProviderIconProps {
	/** Provider ID (e.g. "anthropic", "openai") */
	id: string
	/** Provider display name (used for letter fallback) */
	name: string
	size?: "xs" | "sm" | "md" | "lg"
	className?: string
}

export function ProviderIcon({ id, name, size = "md", className = "" }: ProviderIconProps) {
	const [errored, setErrored] = useState(false)

	const rounding = size === "xs" ? "rounded-sm" : "rounded-md"
	const px = SIZE_PX[size]

	if (!errored) {
		return (
			<img
				src={`https://models.dev/logos/${id}.svg`}
				alt={`${name} logo`}
				width={px}
				height={px}
				className={`shrink-0 object-contain dark:invert ${rounding} ${SIZE_CLASSES[size]} ${className}`}
				aria-hidden="true"
				onError={() => setErrored(true)}
			/>
		)
	}

	// Fallback: colored letter avatar
	const colorClass = AVATAR_COLORS[hashString(id) % AVATAR_COLORS.length]
	const letter = name.charAt(0).toUpperCase()
	const textSize = size === "xs" ? "text-[9px]" : size === "sm" ? "text-xs" : "text-sm"

	return (
		<div
			className={`flex shrink-0 items-center justify-center ${rounding} font-semibold ${SIZE_CLASSES[size]} ${textSize} ${colorClass} ${className}`}
			aria-hidden="true"
		>
			{letter}
		</div>
	)
}
