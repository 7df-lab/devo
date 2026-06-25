import { atom } from "jotai"
import { atomFamily } from "jotai-family"
import type { Todo } from "../lib/types"

/** Per-session todo list */
export const todosFamily = atomFamily((_sessionId: string) => atom<Todo[]>([]))
