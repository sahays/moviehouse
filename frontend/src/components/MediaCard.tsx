import { useState, useEffect } from "react";
import { Square } from "lucide-react";
import type { MediaEntry } from "../types";
import { TranscodeOptions } from "./TranscodeOptions";
import { formatDuration } from "@/lib/formatters";
import { isPlayable, isActivelyTranscoding } from "@/lib/media-helpers";
import { useSettings } from "@/contexts/SettingsContext";
import { MediaCardPoster } from "./media/MediaCardPoster";
import { MediaCardInfo } from "./media/MediaCardInfo";
import { MediaCardActions } from "./media/MediaCardActions";

interface MediaCardProps {
  entry: MediaEntry;
  isPlaying?: boolean;
  onPlay: (entry: MediaEntry) => void;
  onDelete: (id: string) => void;
}

export function MediaCard({
  entry,
  isPlaying,
  onPlay,
  onDelete,
}: MediaCardProps) {
  const [showTranscode, setShowTranscode] = useState(false);
  const [nowSecs, setNowSecs] = useState(() => Math.floor(Date.now() / 1000));
  const { settings } = useSettings();
  const autoTranscode = settings?.auto_transcode ?? true;

  const playable = isPlayable(entry);
  const transcoding = isActivelyTranscoding(entry.transcode_state);
  const progress = transcoding
    ? (entry.transcode_state as { Transcoding: { progress_percent: number } })
        .Transcoding.progress_percent
    : 0;
  const isIndeterminate = transcoding && progress === -1;

  useEffect(() => {
    if (!transcoding) return;
    const interval = setInterval(
      () => setNowSecs(Math.floor(Date.now() / 1000)),
      1000,
    );
    return () => clearInterval(interval);
  }, [transcoding]);

  return (
    <div
      className={`bg-[var(--color-bg-secondary)] border rounded-lg overflow-hidden transition-colors flex ${isPlaying ? "border-blue-500 ring-1 ring-blue-500/30" : "border-[var(--color-border)] hover:border-[var(--color-border-hover)]"}`}
    >
      {/* Poster -- 2:3 aspect ratio, left side */}
      <MediaCardPoster
        entry={entry}
        isPlaying={!!isPlaying}
        playable={playable}
        onPlay={onPlay}
      />

      {/* Details -- right side */}
      <div className="flex-1 min-w-0 p-3 flex flex-col">
        {/* Header: title + context menu */}
        <div className="flex items-start justify-between gap-1">
          <h3 className="text-sm font-semibold text-[var(--color-text-primary)] leading-tight line-clamp-2">
            {entry.episode != null && (
              <span className="text-[var(--color-text-tertiary)] mr-1">
                E{String(entry.episode).padStart(2, "0")}
              </span>
            )}
            {entry.episode_title ?? entry.title}
          </h3>
          <MediaCardActions
            entryId={entry.id}
            groupId={entry.group_id}
            onDelete={onDelete}
            onShowTranscode={() => setShowTranscode(!showTranscode)}
          />
        </div>

        <MediaCardInfo entry={entry} />

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
                return (
                  <div className="flex items-center justify-between mt-1">
                    <span className="text-xs text-[var(--color-text-tertiary)]">
                      {formatDuration(elapsedSecs)} elapsed
                      {etaSecs > 0 && (
                        <span className="ml-1">
                          / ~{formatDuration(etaSecs)} remaining
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

        {entry.transcode_state === "Pending" &&
          !autoTranscode &&
          !entry.group_id && (
            <TranscodeOptions mediaId={entry.id} onStarted={() => {}} />
          )}
        {showTranscode && !entry.group_id && (
          <TranscodeOptions
            mediaId={entry.id}
            onStarted={() => setShowTranscode(false)}
          />
        )}
      </div>
    </div>
  );
}
