import React, { useState } from "react"
import { Provider, createStore } from "jotai"
import type { Automation, AutomationRun } from "../src/preload/api"
import { automationsAtom, automationRunsAtom } from "../src/renderer/atoms/automations"
import { discoveryAtom } from "../src/renderer/atoms/discovery"
import { InboxRunList } from "../src/renderer/components/automations/inbox-run-list"
import { AutomationDetail } from "../src/renderer/components/automations/automation-detail"
import { CreateAutomationDialog } from "../src/renderer/components/automations/create-automation-dialog"

const windows = "C:\\Users\\jack\\WindowsProject\\"
const unix = "/repo/UnixProject/"
const discovered = "C:\\Users\\jack\\DiscoveredProject\\"
const automation = {
 id: "automation", name: "Label review", prompt: "Review project", status: "active",
 workspaces: [windows, unix], schedule: { rrule: "FREQ=DAILY;BYHOUR=9;BYMINUTE=0", timezone: "UTC" },
 execution: {}, nextRunAt: null, runCount: 0, createdAt: 1, updatedAt: 1,
} as Automation
const store = createStore()
store.set(automationsAtom, [automation])
store.set(automationRunsAtom, [windows, unix].map((workspace, i) => ({
 id: `run-${i}`, automationId: "automation", workspace, status: "pending_review",
 attempt: 1, resultTitle: `Result ${i}`, resultSummary: `Summary ${i}`, createdAt: 1, updatedAt: 1,
})) as AutomationRun[])
store.set(discoveryAtom, { ...store.get(discoveryAtom), projects: [{ id: "discovered", worktree: discovered } as any] })
export function AutomationFixture() {
 const [open, setOpen] = useState(false)
 return <Provider store={store}><main className="w-full p-2 bg-background text-foreground">
  <section aria-label="Automation inbox"><InboxRunList onEditAutomation={() => setOpen(true)} onRunNow={() => {}} onTogglePause={() => {}} onDeleteAutomation={() => {}} /></section>
  <section aria-label="Automation detail"><AutomationDetail /></section>
  <button onClick={() => setOpen(true)}>Edit label fixture</button>
  <CreateAutomationDialog open={open} onOpenChange={setOpen} editAutomation={automation} />
  <output id="saved-workspaces" aria-label="Saved payload" />
 </main></Provider>
}
