import React, { useState } from "react"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { SkillPickerDialog } from "../src/renderer/components/chat/skill-picker-dialog"

export function SkillImeFixture() {
 const [client] = useState(() => {
  const queryClient = new QueryClient()
  // A null directory disables network fetching; cached skills are controlled input data.
  queryClient.setQueryData(["skills", null], [
   { name: "alpha", description: "First skill", location: "" },
   { name: "beta", description: "Second skill", location: "" },
  ])
  return queryClient
 })
 const [open, setOpen] = useState(false)
 const [selected, setSelected] = useState<string[]>([])
 return <QueryClientProvider client={client}>
  <main className="p-4">
   <button onClick={() => setOpen(true)}>Choose skill</button>
   <output aria-label="Selections">{JSON.stringify(selected)}</output>
   <SkillPickerDialog open={open} onOpenChange={setOpen} directory={null}
    onSelect={name => setSelected(items => [...items, name])} />
  </main>
 </QueryClientProvider>
}
