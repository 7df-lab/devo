import React, { useState } from "react"
import type { Automation } from "../src/preload/api"
import { AutomationRow } from "../src/renderer/components/automations/automation-row"

const automation = {
 id: "edit-access", name: "Scheduled review", workspaces: ["/repo/devo"],
 status: "active", nextRunAt: Date.now() + 3600000,
} as Automation
export function AutomationEditFixture() {
 const [opened, setOpened] = useState(0)
 const [edited, setEdited] = useState(0)
 return <main className="w-full p-2 bg-background text-foreground">
  <AutomationRow automation={automation} isSelected={false}
   onClick={() => setOpened(n => n + 1)} onEdit={() => setEdited(n => n + 1)}
   onRunNow={() => {}} onTogglePause={() => {}} onDelete={() => {}} />
  <output aria-label="Navigation count">{opened}</output>
  <output aria-label="Edit count">{edited}</output>
 </main>
}
