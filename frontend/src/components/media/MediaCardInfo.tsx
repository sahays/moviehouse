import { Clapperboard, Users, FileText } from "lucide-react";
import type { MediaEntry, TranscodeState } from "../../types";
import { formatBytes } from "@/lib/formatters";
import {
  getStateLabel,
  getStateBadgeClasses,
  isActivelyTranscoding,
} from "@/lib/media-helpers";

interface MediaCardInfoProps {
  entry: MediaEntry;
}

export function MediaCardInfo({ entry }: MediaCardInfoProps) {
  const transcoding = isActivelyTranscoding(entry.transcode_state);
  const stateLabel = getStateLabel(entry.transcode_state);

  return (
    <>
      {/* Meta row */}
      <div className="flex items-center gap-2 mt-1 text-xs text-[var(--color-text-tertiary)]">
        {entry.year && <span>{entry.year}</span>}
        {entry.rating != null && (
          <span className="px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-400 font-medium">
            {entry.rating.toFixed(1)}
          </span>
        )}
        <span>{formatBytes(entry.file_size)}</span>
      </div>

      {/* Director */}
      {entry.director && (
        <div className="flex items-center gap-1.5 mt-1.5 text-xs text-[var(--color-text-tertiary)]">
          <Clapperboard size={12} className="shrink-0" />
          <span className="truncate">{entry.director}</span>
        </div>
      )}

      {/* Cast */}
      {entry.cast.length > 0 && (
        <div className="flex items-start gap-1.5 mt-1 text-xs text-[var(--color-text-tertiary)]">
          <Users size={12} className="shrink-0 mt-0.5" />
          <span className="line-clamp-1">{entry.cast.join(", ")}</span>
        </div>
      )}

      {/* Overview */}
      {entry.overview && (
        <div className="flex items-start gap-1.5 mt-1.5 text-xs text-[var(--color-text-tertiary)]">
          <FileText size={12} className="shrink-0 mt-0.5" />
          <p className="line-clamp-2 leading-relaxed">{entry.overview}</p>
        </div>
      )}

      {/* Version pills */}
      <div className="flex flex-wrap gap-1 mt-2">
        {Object.keys(entry.versions).some((k) => k.startsWith("compat-")) && (
          <span className="px-1.5 py-0.5 rounded text-[10px] font-medium bg-emerald-500/15 text-emerald-400">
            Compatible
          </span>
        )}
        {Object.keys(entry.versions).some((k) => k.startsWith("quality-")) && (
          <span className="px-1.5 py-0.5 rounded text-[10px] font-medium bg-blue-500/15 text-blue-400">
            Quality
          </span>
        )}
        {entry.transcode_state === "Skipped" &&
          Object.keys(entry.versions).length === 0 && (
            <span className="px-1.5 py-0.5 rounded text-[10px] font-medium bg-emerald-500/15 text-emerald-400">
              Direct Play
            </span>
          )}
        {entry.video_codec && (
          <span className="px-1.5 py-0.5 rounded text-[10px] font-medium bg-[var(--color-bg-tertiary)] text-[var(--color-text-tertiary)]">
            {entry.video_codec === "hevc" || entry.video_codec === "h265"
              ? "H.265"
              : entry.video_codec === "h264"
                ? "H.264"
                : entry.video_codec.toUpperCase()}
          </span>
        )}
        {Object.keys(entry.versions).length === 0 &&
          entry.transcode_state !== "Skipped" &&
          !transcoding && (
            <span className="px-1.5 py-0.5 rounded text-[10px] font-medium bg-[var(--color-bg-tertiary)] text-[var(--color-text-tertiary)]">
              {entry.media_file.match(/\.(\w+)$/)?.[1]?.toUpperCase() ?? "?"}
            </span>
          )}
      </div>

      {/* Spacer to push status to bottom */}
      <div className="flex-1" />

      {/* Transcode status */}
      <div className="flex justify-end mt-3">
        {(() => {
          const hasVersions = Object.keys(entry.versions).length > 0;
          const effectiveState =
            hasVersions && !transcoding ? "Ready" : stateLabel;
          const effectiveBadgeState =
            hasVersions && !transcoding
              ? ({ Ready: { output_path: "" } } as TranscodeState)
              : entry.transcode_state;
          return (
            <span className={getStateBadgeClasses(effectiveBadgeState)}>
              {effectiveState}
            </span>
          );
        })()}
      </div>
    </>
  );
}
