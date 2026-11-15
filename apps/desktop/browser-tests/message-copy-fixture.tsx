import React from "react"
import { Toaster } from "sonner"
import { UserMessageBlock } from "../src/renderer/components/chat/user-message-block"
export function MessageCopyFixture() {
 return <main className="w-full p-8 bg-background text-foreground">
  <UserMessageBlock text="Copy this exact message: 项目 review" />
  <Toaster />
 </main>
}
