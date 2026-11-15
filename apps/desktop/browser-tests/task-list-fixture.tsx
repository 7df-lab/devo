import React, { useState } from "react"
import { createStore, Provider } from "jotai"
import { todosFamily } from "../src/renderer/atoms/todos"
import { SessionTaskList } from "../src/renderer/components/chat/session-task-list"
export function TaskListFixture() {
 const [store] = useState(() => {
  const store = createStore()
  store.set(todosFamily("task-fixture"), [
   { content: "First task", status: "pending", priority: "medium" },
   { content: "Second task", status: "inProgress", priority: "medium" },
   { content: "Third task", status: "completed", priority: "medium" },
   { content: "Fourth task", status: "cancelled", priority: "medium" },
  ])
  return store
 })
 return <Provider store={store}><main className="p-4"><SessionTaskList sessionId="task-fixture" /></main></Provider>
}
