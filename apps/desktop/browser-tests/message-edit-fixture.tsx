import React, { useRef, useState } from "react"
import { Toaster } from "sonner"
import { UserMessageBlock } from "../src/renderer/components/chat/user-message-block"

export function MessageEditFixture() {
 const [text, setText] = useState("Original message")
 const [submitted, setSubmitted] = useState<string[]>([])
 const pending = useRef<{ resolve: () => void; reject: () => void } | null>(null)
 return <main className="w-full p-8 bg-background text-foreground">
  <UserMessageBlock text={text} canEdit onEdit={async next => {
   setSubmitted(items => [...items, next])
   await new Promise<void>((resolve, reject) => {
    pending.current = { resolve, reject: () => reject(new Error("Test save rejection")) }
   })
   setText(next)
  }} />
  <div className="mt-16">
   <button onClick={() => { pending.current?.reject(); pending.current = null }}>Reject request</button>
   <button onClick={() => { pending.current?.resolve(); pending.current = null }}>Resolve request</button>
   <output aria-label="Submitted">{JSON.stringify(submitted)}</output>
  </div>
  <Toaster />
 </main>
}
