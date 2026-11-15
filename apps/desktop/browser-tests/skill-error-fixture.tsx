import React, { useState } from "react"
import { QueryClientProvider } from "@tanstack/react-query"
import { queryClient } from "../src/renderer/lib/query-client"
import { SkillPickerDialog } from "../src/renderer/components/chat/skill-picker-dialog"
export function SkillErrorFixture() {
 const [open, setOpen] = useState(false)
 const [selected, setSelected] = useState("")
 return <QueryClientProvider client={queryClient}>
  <main className="p-4">
   <button onClick={() => setOpen(true)}>Choose skill</button>
   <output aria-label="Selected skill">{selected}</output>
   <SkillPickerDialog open={open} onOpenChange={setOpen} directory="/skill-error-fixture" onSelect={setSelected} />
  </main>
 </QueryClientProvider>
}
