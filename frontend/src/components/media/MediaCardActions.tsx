import { MoreVertical, RefreshCw, RotateCcw, Trash2 } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

interface MediaCardActionsProps {
  entryId: string;
  groupId: string | null;
  onDelete: (id: string) => void;
  onShowTranscode: () => void;
}

export function MediaCardActions({
  entryId,
  onDelete,
  onShowTranscode,
}: MediaCardActionsProps) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger className="shrink-0 p-1 rounded text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-tertiary)] transition-colors">
        <MoreVertical size={16} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-[180px]">
        <DropdownMenuItem onClick={onShowTranscode}>
          <RefreshCw size={14} />
          Re-transcode
        </DropdownMenuItem>
        <DropdownMenuItem
          onClick={() => {
            fetch(`/api/v1/library/${entryId}/refresh`, {
              method: "POST",
            }).catch(() => {});
          }}
        >
          <RotateCcw size={14} />
          Refresh metadata
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          className="text-red-400 focus:text-red-400"
          onClick={() => onDelete(entryId)}
        >
          <Trash2 size={14} />
          Remove
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
