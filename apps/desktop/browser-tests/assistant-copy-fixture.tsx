import React from "react"
import { Toaster } from "sonner"
import { StickToBottom } from "use-stick-to-bottom"
import { ChatTurnComponent } from "../src/renderer/components/chat/chat-turn"
import type { ChatTurn } from "../src/renderer/atoms/derived/session-chat"
const common = { sessionId: "copy", turnId: "turn", revision: 1, state: "completed", createdAt: "2026-11-15T00:00:00Z", updatedAt: "2026-11-15T00:00:00Z" }
const turn: ChatTurn = {
 id: "user", turnId: "turn",
 userMessage: { info: { ...common, id: "user", seq: 1, item: { type: "userMessage", content: [{ type: "text", text: "Question" }], entry: "turnStart" } } },
 assistantMessages: [{ info: { ...common, id: "assistant", seq: 2, item: { type: "assistantMessage", text: "Exact assistant response: 项目 review" } } }],
}
export function AssistantCopyFixture() {
 return <main className="p-4"><StickToBottom><ChatTurnComponent turn={turn} isLast isWorking={false} /></StickToBottom><Toaster /></main>
}
