import { Play } from "lucide-react";
import type { MediaEntry } from "../types";
import { formatBytes, formatPlaybackTime } from "@/lib/formatters";

interface EpisodeRowProps {
  entry: MediaEntry;
  isPlaying: boolean;
  onPlay: (entry: MediaEntry) => void;
}

export function EpisodeRow({ entry, isPlaying, onPlay }: EpisodeRowProps) {
  const playable =
    entry.transcode_state === "Skipped" ||
    (typeof entry.transcode_state === "object" &&
      "Ready" in entry.transcode_state) ||
    Object.keys(entry.versions).length > 0;

  const transcoding =
    typeof entry.transcode_state === "object" &&
    "Transcoding" in entry.transcode_state;
  const progress = transcoding
    ? (entry.transcode_state as { Transcoding: { progress_percent: number } })
        .Transcoding.progress_percent
    : 0;

  return (
    <div
      className={`flex items-center gap-3 px-3 py-2 rounded-md transition-colors ${
        isPlaying ? "bg-blue-500/10" : "hover:bg-[var(--color-bg-tertiary)]"
      } ${playable ? "cursor-pointer" : ""}`}
      onClick={() => playable && onPlay(entry)}
      role={playable ? "button" : undefined}
      tabIndex={playable ? 0 : undefined}
      onKeyDown={(e) => {
        if (playable && (e.key === "Enter" || e.key === " ")) {
          e.preventDefault();
          onPlay(entry);
        }
      }}
    >
      {/* Episode number */}
      <span className="text-xs text-[var(--color-text-tertiary)] w-8 shrink-0 text-right">
        {entry.episode != null
          ? `E${String(entry.episode).padStart(2, "0")}`
          : "\u2014"}
      </span>

      {/* Play indicator or icon */}
      {isPlaying ? (
        <div className="flex items-center gap-0.5 w-4 shrink-0">
          <span className="w-0.5 h-2 bg-blue-400 rounded-full animate-pulse" />
          <span className="w-0.5 h-3 bg-blue-400 rounded-full animate-pulse [animation-delay:150ms]" />
          <span className="w-0.5 h-2 bg-blue-400 rounded-full animate-pulse [animation-delay:300ms]" />
        </div>
      ) : playable ? (
        <Play
          size={12}
          className="text-[var(--color-text-tertiary)] shrink-0"
        />
      ) : (
        <div className="w-4 shrink-0" />
      )}

      {/* Title */}
      <span
        className={`flex-1 text-sm truncate ${isPlaying ? "text-blue-400 font-medium" : "text-[var(--color-text-primary)]"}`}
      >
        {entry.episode_title || entry.title}
      </span>

      {/* Status */}
      {transcoding && (
        <span className="text-xs text-blue-400 shrink-0">
          {progress > 0 ? `${progress.toFixed(0)}%` : "..."}
        </span>
      )}

      {/* Play progress */}
      {!transcoding &&
        entry.play_position != null &&
        entry.duration != null &&
        entry.duration > 0 && (
          <span className="text-xs text-[var(--color-text-tertiary)] shrink-0">
            {formatPlaybackTime(entry.play_position)} /{" "}
            {formatPlaybackTime(entry.duration)}
          </span>
        )}

      {/* Size */}
      <span className="text-xs text-[var(--color-text-tertiary)] shrink-0">
        {formatBytes(entry.file_size)}
      </span>
    </div>
  );
}
