/**
 * A lightweight replacement for @radix-ui/react-use-controllable-state.
 *
 * Manages a value that can be either controlled (via `prop`) or uncontrolled
 * (via internal state initialized from `defaultProp`). Calls `onChange` whenever
 * the value changes.
 */

import type { Dispatch, SetStateAction } from "react"
import { useCallback, useEffect, useRef, useState } from "react"

interface UseControllableStateParams<T> {
	prop?: T | undefined
	defaultProp: T
	onChange?: (state: T) => void
}

function useControllableState<T>({
	prop,
	defaultProp,
	onChange,
}: UseControllableStateParams<T>): [T, Dispatch<SetStateAction<T>>] {
	const [uncontrolledValue, setUncontrolledValue] = useState<T>(defaultProp)
	const isControlled = prop !== undefined
	const value = isControlled ? prop : uncontrolledValue

	// Use a ref for onChange so we always call the latest version without
	// needing it in dependency arrays (mirrors Radix's useInsertionEffect approach).
	const onChangeRef = useRef(onChange)
	useEffect(() => {
		onChangeRef.current = onChange
	}, [onChange])

	// Track previous value to fire onChange only when it actually changes.
	const prevValueRef = useRef(value)
	useEffect(() => {
		if (prevValueRef.current !== uncontrolledValue) {
			onChangeRef.current?.(uncontrolledValue)
			prevValueRef.current = uncontrolledValue
		}
	}, [uncontrolledValue])

	const setValue: Dispatch<SetStateAction<T>> = useCallback(
		(nextValue) => {
			if (isControlled) {
				const resolved =
					typeof nextValue === "function" ? (nextValue as (prev: T) => T)(prop as T) : nextValue
				if (!Object.is(resolved, prop)) {
					onChangeRef.current?.(resolved)
				}
			} else {
				setUncontrolledValue(nextValue)
			}
		},
		[isControlled, prop],
	)

	return [value, setValue]
}

export { useControllableState }
export type { UseControllableStateParams }
