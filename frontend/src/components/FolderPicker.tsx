import { useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Folder, FolderUp, Check, FolderOpen, HardDrive } from "lucide-react";

interface FolderPickerProps {
  onSelect: (path: string) => void;
}

interface BrowseResult {
  current: string;
  parent: string | null;
  dirs: string[];
}

export function FolderPicker({ onSelect }: FolderPickerProps) {
  const [open, setOpen] = useState(false);
  const [result, setResult] = useState<BrowseResult | null>(null);
  const [loading, setLoading] = useState(false);

  const browse = async (path?: string) => {
    setLoading(true);
    try {
      const url = path
        ? `/api/v1/filesystem/browse?path=${encodeURIComponent(path)}`
        : "/api/v1/filesystem/browse";
      const res = await fetch(url);
      const data = await res.json();
      if (data && Array.isArray(data.dirs)) {
        setResult(data);
      }
      // else: API error response, keep showing current result
    } catch {
      // Network error
    }
    setLoading(false);
  };

  const handleOpen = () => {
    setOpen(true);
    browse();
  };

  const handleSelect = () => {
    if (result) {
      onSelect(result.current);
      setOpen(false);
    }
  };

  return (
    <>
      <Button
        variant="outline"
        size="icon"
        onClick={handleOpen}
        title="Browse server folders"
      >
        <FolderOpen size={16} />
      </Button>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-2xl w-[90vw]">
          <DialogHeader>
            <DialogTitle>Select Folder</DialogTitle>
          </DialogHeader>

          {result && (
            <div className="overflow-hidden">
              <div className="flex items-center gap-2 mb-2">
                <div
                  className="text-xs text-muted-foreground font-mono truncate px-1 flex-1"
                  title={result.current}
                >
                  {result.current}
                </div>
                <button
                  onClick={() => browse("/Volumes")}
                  className="flex items-center gap-1 px-2 py-1 text-xs text-muted-foreground hover:text-foreground hover:bg-accent/50 rounded transition-colors shrink-0"
                  title="Browse external drives"
                >
                  <HardDrive size={12} />
                  Drives
                </button>
              </div>

              <div className="max-h-72 overflow-y-auto overflow-x-hidden border rounded-md">
                {result.parent != null && (
                  <button
                    onClick={() => browse(result.parent ?? undefined)}
                    className="flex items-center gap-2 w-full px-3 py-2 text-sm text-muted-foreground hover:bg-accent/50 transition-colors border-b"
                  >
                    <FolderUp size={16} />
                    <span>..</span>
                  </button>
                )}
                {result.dirs.length === 0 && (
                  <div className="px-3 py-4 text-xs text-muted-foreground text-center">
                    No subdirectories
                  </div>
                )}
                {result.dirs.map((dir) => (
                  <button
                    key={dir}
                    onClick={() => browse(`${result.current}/${dir}`)}
                    className="flex items-center gap-2 w-full px-3 py-2 text-sm hover:bg-accent/50 transition-colors min-w-0"
                  >
                    <Folder size={16} className="text-blue-400 shrink-0" />
                    <span className="truncate text-left">{dir}</span>
                  </button>
                ))}
              </div>
            </div>
          )}

          {loading && (
            <div className="text-xs text-muted-foreground text-center py-4">
              Loading...
            </div>
          )}

          <DialogFooter className="gap-2">
            <Button variant="outline" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button onClick={handleSelect} disabled={!result}>
              <Check size={16} />
              Select This Folder
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
