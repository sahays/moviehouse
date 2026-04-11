import { useState, useEffect } from "react";
import {
  MoreVertical,
  Play,
  Trash2,
  RefreshCw,
  RotateCcw,
  Square,
  Clapperboard,
  Users,
  FileText,
} from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { MediaEntry, TranscodeState } from "../types";
import { TranscodeOptions } from "./TranscodeOptions";

interface MediaCardProps {
  entry: MediaEntry;
  onPlay: (entry: MediaEntry) => void;
  onDelete: (id: string) => void;
}

function formatSize(bytes: number): string {
  if (bytes >= 1073741824) return (bytes / 1073741824).toFixed(1) + " GB";
  if (bytes >= 1048576) return (bytes / 1048576).toFixed(0) + " MB";
  return (bytes / 1024).toFixed(0) + " KB";
}

function getStateLabel(state: TranscodeState): string {
  if (state === "Skipped" || state === "Pending" || state === "Unavailable")
    return state;
  if (typeof state === "object") {
    if ("Transcoding" in state) {
      const pct = state.Transcoding.progress_percent;
      if (pct === -1) return "Transcoding...";
      if (pct === 0) return "Starting...";
      return `Transcoding ${pct.toFixed(0)}%`;
    }
    if ("Ready" in state) return "Ready";
    if ("Failed" in state) return "Failed";
  }
  return "Unknown";
}

function isPlayable(entry: MediaEntry): boolean {
  if (entry.transcode_state === "Skipped") return true;
  if (
    typeof entry.transcode_state === "object" &&
    "Ready" in entry.transcode_state
  )
    return true;
  if (Object.keys(entry.versions).length > 0) return true;
  return false;
}

function isActivelyTranscoding(state: TranscodeState): boolean {
  return typeof state === "object" && "Transcoding" in state;
}

function getStateBadgeClasses(state: TranscodeState): string {
  const base = "px-2 py-0.5 rounded text-xs font-medium";
  if (state === "Skipped" || (typeof state === "object" && "Ready" in state))
    return `${base} bg-emerald-500/15 text-emerald-400`;
  if (typeof state === "object" && "Transcoding" in state)
    return `${base} bg-blue-500/15 text-blue-400`;
  if (state === "Pending") return `${base} bg-gray-500/15 text-gray-400`;
  if (state === "Unavailable") return `${base} bg-amber-500/15 text-amber-400`;
  if (typeof state === "object" && "Failed" in state)
    return `${base} bg-red-500/15 text-red-400`;
  return `${base} bg-gray-500/15 text-gray-400`;
}

export function MediaCard({ entry, onPlay, onDelete }: MediaCardProps) {
  const [showTranscode, setShowTranscode] = useState(false);
  const [autoTranscode, setAutoTranscode] = useState(true);
  const [nowSecs, setNowSecs] = useState(() => Math.floor(Date.now() / 1000));

  const playable = isPlayable(entry);
  const stateLabel = getStateLabel(entry.transcode_state);
  const transcoding = isActivelyTranscoding(entry.transcode_state);
  const progress = transcoding
    ? (entry.transcode_state as { Transcoding: { progress_percent: number } })
        .Transcoding.progress_percent
    : 0;
  const isIndeterminate = transcoding && progress === -1;

  useEffect(() => {
    fetch("/api/v1/settings")
      .then((r) => r.json())
      .then((s) => setAutoTranscode(s.auto_transcode))
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (!transcoding) return;
    const interval = setInterval(
      () => setNowSecs(Math.floor(Date.now() / 1000)),
      1000,
    );
    return () => clearInterval(interval);
  }, [transcoding]);

  return (
    <div className="bg-[var(--color-bg-secondary)] border border-[var(--color-border)] rounded-lg overflow-hidden hover:border-[var(--color-border-hover)] transition-colors flex">
      {/* Poster — 2:3 aspect ratio, left side */}
      <div
        className={`relative w-40 sm:w-44 shrink-0 aspect-[2/3] bg-gradient-to-br from-blue-900/40 to-cyan-900/30 flex items-center justify-center overflow-hidden group ${playable ? "cursor-pointer" : ""}`}
        onClick={() => playable && onPlay(entry)}
      >
        {entry.poster_url ? (
          <img
            src={entry.poster_url}
            alt={entry.title}
            className="absolute inset-0 w-full h-full object-cover"
          />
        ) : (
          <span className="text-5xl font-bold text-white/20">
            {entry.title.charAt(0).toUpperCase()}
          </span>
        )}
        {playable && (
          <div className="absolute inset-0 bg-black/0 group-hover:bg-black/40 transition-colors flex items-center justify-center">
            <Play
              size={36}
              className="text-white/0 group-hover:text-white/90 transition-colors fill-current"
            />
          </div>
        )}
      </div>

      {/* Details — right side */}
      <div className="flex-1 min-w-0 p-3 flex flex-col">
        {/* Header: title + context menu */}
        <div className="flex items-start justify-between gap-1">
          <h3 className="text-sm font-semibold text-[var(--color-text-primary)] leading-tight line-clamp-2">
            {entry.title}
          </h3>
          <DropdownMenu>
            <DropdownMenuTrigger className="shrink-0 p-1 rounded text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-tertiary)] transition-colors">
              <MoreVertical size={16} />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="min-w-[180px]">
              <DropdownMenuItem
                onClick={() => setShowTranscode(!showTranscode)}
              >
                <RefreshCw size={14} />
                Re-transcode
              </DropdownMenuItem>
              <DropdownMenuItem
                onClick={() => {
                  fetch(`/api/v1/library/${entry.id}/refresh`, {
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
                onClick={() => onDelete(entry.id)}
              >
                <Trash2 size={14} />
                Remove
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        {/* Meta row */}
        <div className="flex items-center gap-2 mt-1 text-xs text-[var(--color-text-tertiary)]">
          {entry.year && <span>{entry.year}</span>}
          {entry.rating != null && (
            <span className="px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-400 font-medium">
              {entry.rating.toFixed(1)}
            </span>
          )}
          <span>{formatSize(entry.file_size)}</span>
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
          {Object.keys(entry.versions).some((k) =>
            k.startsWith("quality-"),
          ) && (
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

        {/* Transcode progress */}
        {transcoding && (
          <>
            <div className="h-1 bg-[var(--color-bg-tertiary)] rounded-full mt-1 overflow-hidden">
              {isIndeterminate ? (
                <div className="h-full bg-blue-500 rounded-full animate-pulse w-full" />
              ) : (
                <div
                  className="h-full bg-blue-500 rounded-full transition-all"
                  style={{ width: `${progress}%` }}
                />
              )}
            </div>
            {entry.transcode_started_at &&
              (() => {
                const elapsedSecs = Math.max(
                  0,
                  nowSecs - entry.transcode_started_at,
                );
                const etaSecs = Math.max(
                  0,
                  progress > 1
                    ? Math.floor((elapsedSecs * (100 - progress)) / progress)
                    : 0,
                );
                const fmtDur = (s: number) => {
                  if (s < 60) return `${s}s`;
                  const m = Math.floor(s / 60);
                  if (m < 60) return `${m}m ${s % 60}s`;
                  return `${Math.floor(m / 60)}h ${m % 60}m`;
                };
                return (
                  <div className="flex items-center justify-between mt-1">
                    <span className="text-xs text-[var(--color-text-tertiary)]">
                      {fmtDur(elapsedSecs)} elapsed
                      {etaSecs > 0 && (
                        <span className="ml-1">
                          / ~{fmtDur(etaSecs)} remaining
                        </span>
                      )}
                    </span>
                    <button
                      className="flex items-center gap-1 px-2 py-0.5 rounded text-[10px] font-medium text-red-400 hover:bg-red-500/10 transition-colors"
                      onClick={(e) => {
                        e.stopPropagation();
                        fetch(`/api/v1/library/${entry.id}/cancel-transcode`, {
                          method: "POST",
                        }).catch(() => {});
                      }}
                    >
                      <Square size={10} className="fill-current" />
                      Stop
                    </button>
                  </div>
                );
              })()}
          </>
        )}

        {entry.transcode_state === "Pending" && !autoTranscode && (
          <TranscodeOptions
            mediaId={entry.id}
            videoCodec={entry.video_codec}
            onStarted={() => {}}
          />
        )}
        {showTranscode && (
          <TranscodeOptions
            mediaId={entry.id}
            videoCodec={entry.video_codec}
            onStarted={() => setShowTranscode(false)}
          />
        )}
      </div>
    </div>
  );
}
