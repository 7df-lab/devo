import React, { useState } from "react"
import {
 SearchableListPopover, SearchableListPopoverTrigger, SearchableListPopoverContent,
 SearchableListPopoverSearch, SearchableListPopoverList, SearchableListPopoverItem,
 SearchableListPopoverEmpty, useSearchableListPopoverSearch,
} from "@devo/ui/components/searchable-list-popover"
import { ModelPickerList } from "../src/renderer/components/chat/model-picker-list"

function BranchResults({ select }: { select: (value: string) => void }) {
 const search = useSearchableListPopoverSearch()
 const rows = ["alpha", "beta"].filter(name => name.includes(search))
 return <SearchableListPopoverList>{rows.length ? rows.map(name =>
  <SearchableListPopoverItem key={name} onSelect={() => select(name)}>{name}</SearchableListPopoverItem>
 ) : <SearchableListPopoverEmpty />}</SearchableListPopoverList>
}
export function SearchFixture() {
 const [open, setOpen] = useState(false)
 const [selected, setSelected] = useState("")
 const modelMode = new URLSearchParams(location.search).get("picker") === "model"
 const select = (value: string) => { setSelected(value); setOpen(false) }
 const models = ["alpha", "beta"].map(name => ({value: `local/${name}`, selection: {providerID:"local",modelID:name}, providerID:"local",providerName:"Local",modelID:name,displayName:name,ready:true}))
 return <main className="p-4 pt-40">
  <SearchableListPopover open={open} onOpenChange={setOpen}>
   <SearchableListPopoverTrigger>Choose {modelMode ? "model" : "branch"}</SearchableListPopoverTrigger>
   <SearchableListPopoverContent side="bottom" align="start">
    {modelMode ? <ModelPickerList models={models} activeValue={null} loading={false} error={null}
      onRetry={() => {}} onSelect={model => select(model.modelID)} onManageProviders={() => {}} /> : <>
      <SearchableListPopoverSearch placeholder="Search branches..." />
      <BranchResults select={select} />
    </>}
   </SearchableListPopoverContent>
  </SearchableListPopover>
  <button type="button">Outside</button>
  <output aria-label="Selection">{selected}</output>
 </main>
}
