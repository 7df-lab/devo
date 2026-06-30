import { cn } from "@devo/ui/lib/utils";
import { BubblesIcon, PackageCheckIcon } from "lucide-react";
import type { SessionCompactionStatus } from "../../atoms/compaction";

export const COMPACTION_STARTED_TEXT = "Session compaction started.";

export function isCompactionStatusText(text: string): boolean {
  return text.trim() === COMPACTION_STARTED_TEXT;
}

export function CompactionStatusDivider({
  status,
  className,
}: {
  status: SessionCompactionStatus;
  className?: string;
}) {
  const isCompleted = status === "completed";
  const Icon = isCompleted ? PackageCheckIcon : BubblesIcon;
  const label = isCompleted ? "Context compacted" : "Compacting context";

  return (
    <div
      role="status"
      aria-label={label}
      className={cn(
        "flex items-center gap-2 py-1.5 text-muted-foreground",
        className,
      )}
    >
      <div className="h-px flex-1 bg-border/70" aria-hidden="true" />
      <div className="inline-flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground/80">
        <Icon className="size-3.5 stroke-[1.5]" aria-hidden="true" />
        <span>{label}</span>
      </div>
      <div className="h-px flex-1 bg-border/70" aria-hidden="true" />
    </div>
  );
}
