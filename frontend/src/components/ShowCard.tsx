import { useState } from "react";
import { ChevronDown, ChevronUp, Wand2, Square, FileText } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { MediaEntry, MediaGroup } from "../types";
import { formatBytes } from "@/lib/formatters";
import { EpisodeRow } from "./EpisodeRow";

interface ShowCardProps {
  group: MediaGroup;
  playingId: string | null;
  onPlay: (entry: MediaEntry) => void;
  onSelect?: () => void;
}

export function ShowCard({
  group,
  playingId,
  onPlay,
  onSelect,
}: ShowCardProps) {
  const [expanded, setExpanded] = useState(false);
  const [transcoding, setTranscoding] = useState(false);

  // Group episodes by season
  const seasons = new Map<number, MediaEntry[]>();
  for (const entry of group.entries) {
    const s = entry.season ?? 0;
    const existing = seasons.get(s) ?? [];
    existing.push(entry);
    seasons.set(s, existing);
  }
  // Sort seasons
  const sortedSeasons = [...seasons.entries()].sort(([a], [b]) => a - b);

  const totalSize = group.entries.reduce((sum, e) => sum + e.file_size, 0);
  const readyCount = group.entries.filter(
    (e) =>
      e.transcode_state === "Skipped" ||
      (typeof e.transcode_state === "object" && "Ready" in e.transcode_state) ||
      Object.keys(e.versions).length > 0,
  ).length;
  const transcodingCount = group.entries.filter(
    (e) =>
      typeof e.transcode_state === "object" &&
      "Transcoding" in e.transcode_state,
  ).length;

  const handleTranscodeAll = async () => {
    setTranscoding(true);
    try {
      await fetch(`/api/v1/library/groups/${group.group_id}/transcode-all`, {
        method: "POST",
      });
    } catch {
      /* ignore */
    }
    setTranscoding(false);
  };

  const handleStopAll = async () => {
    await fetch(`/api/v1/library/groups/${group.group_id}/stop-all`, {
      method: "POST",
    }).catch(() => {});
  };

  return (
    <div
      className={`bg-[var(--color-bg-secondary)] border rounded-lg overflow-hidden transition-colors ${
        playingId && group.entries.some((e) => e.id === playingId)
          ? "border-blue-500 ring-1 ring-blue-500/30"
          : "border-[var(--color-border)] hover:border-[var(--color-border-hover)]"
      }`}
    >
      <div className="flex">
        {/* Poster — clickable to drill down */}
        <div
          className="relative w-40 sm:w-44 shrink-0 aspect-[2/3] bg-gradient-to-br from-blue-900/40 to-cyan-900/30 flex items-center justify-center overflow-hidden cursor-pointer group"
          onClick={onSelect}
          role="button"
          tabIndex={0}
          onKeyDown={(e) => {
            if ((e.key === "Enter" || e.key === " ") && onSelect) {
              e.preventDefault();
              onSelect();
            }
          }}
        >
          {group.poster_url ? (
            <img
              src={group.poster_url}
              alt={group.title}
              className="absolute inset-0 w-full h-full object-cover"
            />
          ) : (
            <span className="text-5xl font-bold text-white/20">
              {group.title.charAt(0).toUpperCase()}
            </span>
          )}
          <div className="absolute inset-0 bg-black/0 group-hover:bg-black/20 transition-colors" />
        </div>

        {/* Details */}
        <div className="flex-1 min-w-0 p-3 flex flex-col">
          <button
            type="button"
            className="text-sm font-semibold text-[var(--color-text-primary)] leading-tight line-clamp-2 cursor-pointer hover:text-blue-400 transition-colors text-left p-0 bg-transparent border-none"
            onClick={onSelect}
          >
            {group.show_name || group.title}
          </button>

          <div className="flex items-center gap-2 mt-1 text-xs text-[var(--color-text-tertiary)]">
            {group.season_count > 0 && (
              <span>
                {group.season_count} Season{group.season_count > 1 ? "s" : ""}
              </span>
            )}
            <span>
              {group.episode_count} Episode{group.episode_count > 1 ? "s" : ""}
            </span>
            {group.rating != null && (
              <span className="px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-400 font-medium">
                {group.rating.toFixed(1)}
              </span>
            )}
            <span>{formatBytes(totalSize)}</span>
          </div>

          {group.overview && (
            <div className="flex items-start gap-1.5 mt-1.5 text-xs text-[var(--color-text-tertiary)]">
              <FileText size={12} className="shrink-0 mt-0.5" />
              <p className="line-clamp-2 leading-relaxed">{group.overview}</p>
            </div>
          )}

          {/* Progress summary */}
          <div className="flex items-center gap-2 mt-2 text-xs">
            <span className="text-emerald-400">
              {readyCount}/{group.episode_count} ready
            </span>
            {transcodingCount > 0 && (
              <span className="text-blue-400">
                {transcodingCount} transcoding
              </span>
            )}
          </div>

          <div className="flex-1" />

          {/* Actions */}
          <div className="flex items-center gap-2 mt-2">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setExpanded(!expanded)}
            >
              {expanded ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
              {expanded ? "Collapse" : "Episodes"}
            </Button>
            {transcodingCount > 0 ? (
              <Button
                variant="ghost"
                size="sm"
                className="text-red-400"
                onClick={handleStopAll}
              >
                <Square size={12} className="fill-current" />
                Stop All
              </Button>
            ) : readyCount < group.episode_count ? (
              <Button
                variant="ghost"
                size="sm"
                onClick={handleTranscodeAll}
                disabled={transcoding}
              >
                <Wand2 size={14} />
                {transcoding ? "Queueing..." : "Transcode All"}
              </Button>
            ) : null}
          </div>
        </div>
      </div>

      {/* Expanded episode list */}
      {expanded && (
        <div className="border-t border-[var(--color-border)] p-2">
          {sortedSeasons.map(([seasonNum, episodes]) => (
            <div key={seasonNum}>
              {group.season_count > 1 && (
                <div className="text-xs font-semibold text-[var(--color-text-secondary)] px-3 py-1.5 mt-1 first:mt-0">
                  Season {seasonNum}
                </div>
              )}
              {episodes.map((ep) => (
                <EpisodeRow
                  key={ep.id}
                  entry={ep}
                  isPlaying={playingId === ep.id}
                  onPlay={onPlay}
                />
              ))}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
