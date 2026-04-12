import { useState, useEffect } from "react";
import type { SystemStatus } from "../../types";

export function SystemInfo() {
  const [systemStatus, setSystemStatus] = useState<SystemStatus | null>(null);

  useEffect(() => {
    fetch("/api/v1/system/status")
      .then((r) => r.json())
      .then((data: unknown) => {
        if (data && typeof data === "object")
          setSystemStatus(data as SystemStatus);
      })
      .catch(() => {});
  }, []);

  return (
    <div className="bg-[var(--color-bg-secondary)] border border-[var(--color-border)] rounded-lg p-3">
      <div className="flex justify-between py-1.5 text-sm">
        <span className="text-[var(--color-text-secondary)]">FFmpeg</span>
        <span
          className={
            systemStatus?.ffmpeg_available
              ? "text-emerald-400"
              : "text-amber-400"
          }
        >
          {systemStatus?.ffmpeg_available ? "Installed" : "Not installed"}
        </span>
      </div>
      {systemStatus?.ffmpeg_version && (
        <div className="flex justify-between py-1.5 text-sm">
          <span className="text-[var(--color-text-secondary)]">Version</span>
          <span className="text-[var(--color-text-tertiary)] text-xs">
            {systemStatus.ffmpeg_version.split(" ").slice(0, 3).join(" ")}
          </span>
        </div>
      )}
    </div>
  );
}
