import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { X } from "lucide-react";
import type { SystemStatus } from "../types";

export function FfmpegBanner() {
  const [status, setStatus] = useState<SystemStatus | null>(null);
  const [dismissed, setDismissed] = useState(
    () => sessionStorage.getItem("ffmpeg-banner-dismissed") === "true",
  );

  useEffect(() => {
    fetch("/api/v1/system/status")
      .then((r) => r.json())
      .then(setStatus)
      .catch(() => {});
  }, []);

  if (!status || status.ffmpeg_available || dismissed) return null;

  return (
    <div className="flex items-center justify-between px-4 py-3 bg-[var(--color-bg-tertiary)] border border-[var(--color-border)] rounded-lg mb-4 text-xs text-[var(--color-text-secondary)]">
      <div>
        <strong className="text-amber-400 font-medium">
          FFmpeg is not installed.
        </strong>{" "}
        Some video formats (MKV, AVI) cannot be transcoded for playback.
        <div className="flex flex-wrap gap-3 mt-1">
          <code className="px-2 py-0.5 bg-[var(--color-bg-primary)] rounded text-xs text-[var(--color-text-primary)] font-mono">
            macOS: brew install ffmpeg
          </code>
          <code className="px-2 py-0.5 bg-[var(--color-bg-primary)] rounded text-xs text-[var(--color-text-primary)] font-mono">
            Ubuntu: sudo apt install ffmpeg
          </code>
          <code className="px-2 py-0.5 bg-[var(--color-bg-primary)] rounded text-xs text-[var(--color-text-primary)] font-mono">
            Windows: winget install ffmpeg
          </code>
        </div>
      </div>
      <Button
        variant="ghost"
        size="sm"
        onClick={() => {
          setDismissed(true);
          sessionStorage.setItem("ffmpeg-banner-dismissed", "true");
        }}
      >
        <X size={14} />
      </Button>
    </div>
  );
}
