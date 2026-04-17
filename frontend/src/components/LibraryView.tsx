import { useMemo, useState } from "react";
import { ChevronLeft, Tv, Film, Wand2, RotateCcw, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { MediaEntry, MediaGroup } from "../types";
import { MediaCard } from "./MediaCard";
import { ShowCard } from "./ShowCard";
import { VideoPlayer } from "./VideoPlayer";
import { formatBytes, formatPlaybackTime } from "@/lib/formatters";
import { hasProgress, progressPercent } from "@/lib/media-helpers";
import { useLibraryGroups } from "@/hooks/useLibraryGroups";

interface LibraryViewProps {
  library: MediaEntry[];
  onRefresh: () => void;
}

export function LibraryView({ library, onRefresh }: LibraryViewProps) {
  const [playing, setPlaying] = useState<MediaEntry | null>(null);
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [selectedSeason, setSelectedSeason] = useState<number | null>(null);

  const selectShow = (show: MediaGroup | null) => {
    setSelectedGroupId(show?.group_id ?? null);
    setSelectedSeason(null);
  };

  const handleTranscodeSeason = async () => {
    if (!selectedShow?.group_id) return;
    await fetch(
      `/api/v1/library/groups/${selectedShow.group_id}/transcode-all?season=${activeSeason}`,
      { method: "POST" },
    ).catch(() => {});
  };

  const handleFetchMetadata = async () => {
    if (!selectedShow?.group_id) return;
    await fetch(
      `/api/v1/library/groups/${selectedShow.group_id}/refresh-metadata?season=${activeSeason}`,
      { method: "POST" },
    ).catch(() => {});
  };

  const { shows, movies } = useLibraryGroups(library);

  const continueWatching = useMemo(
    () =>
      library
        .filter((e) => hasProgress(e))
        .sort((a, b) => (b.last_played_at ?? 0) - (a.last_played_at ?? 0)),
    [library],
  );

  // Derive selectedShow from latest library data (stays in sync with polling)
  const selectedShow = selectedGroupId
    ? (shows.find((s) => s.group_id === selectedGroupId) ?? null)
    : null;

  const seasons = selectedShow
    ? [
        ...new Set(
          selectedShow.entries
            .map((ep) => ep.season)
            .filter((s): s is number => s != null),
        ),
      ].sort((a, b) => a - b)
    : [];

  const activeSeason = selectedSeason ?? seasons[0] ?? 0;

  const seasonEpisodes = selectedShow
    ? selectedShow.entries.filter((ep) => ep.season === activeSeason)
    : [];

  const handleDelete = async (id: string) => {
    await fetch(`/api/v1/library/${id}`, { method: "DELETE" });
    onRefresh();
  };

  if (library.length === 0) {
    return (
      <div className="text-center py-16 text-[var(--color-text-tertiary)]">
        <p>Your library is empty.</p>
        <p>Download a movie or show to get started.</p>
      </div>
    );
  }

  // Drilldown: show detail page with episodes as cards
  if (selectedShow) {
    return (
      <>
        <div className="mb-4">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => selectShow(null)}
            className="text-[var(--color-text-secondary)]"
          >
            <ChevronLeft size={16} />
            Back to Library
          </Button>
        </div>

        {/* Show header */}
        <div className="flex gap-4 mb-6">
          <div className="w-32 sm:w-40 shrink-0 aspect-[2/3] rounded-lg overflow-hidden bg-gradient-to-br from-blue-900/40 to-cyan-900/30 flex items-center justify-center">
            {selectedShow.poster_url ? (
              <img
                src={selectedShow.poster_url}
                alt={selectedShow.title}
                className="w-full h-full object-cover"
              />
            ) : (
              <span className="text-4xl font-bold text-white/20">
                {selectedShow.title.charAt(0).toUpperCase()}
              </span>
            )}
          </div>
          <div className="flex-1 min-w-0">
            <h2 className="text-xl font-bold text-[var(--color-text-primary)]">
              {selectedShow.title}
            </h2>
            <div className="flex items-center gap-2 mt-1 text-sm text-[var(--color-text-tertiary)]">
              {selectedShow.season_count > 0 && (
                <span>
                  {selectedShow.season_count} Season
                  {selectedShow.season_count > 1 ? "s" : ""}
                </span>
              )}
              <span>
                {selectedShow.episode_count} Episode
                {selectedShow.episode_count > 1 ? "s" : ""}
              </span>
              {selectedShow.rating != null && (
                <span className="px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-400 font-medium text-xs">
                  {selectedShow.rating.toFixed(1)}
                </span>
              )}
              <span>
                {formatBytes(
                  selectedShow.entries.reduce((s, e) => s + e.file_size, 0),
                )}
              </span>
            </div>
            {selectedShow.overview && (
              <p className="text-sm text-[var(--color-text-tertiary)] mt-2 line-clamp-3">
                {selectedShow.overview}
              </p>
            )}
          </div>
        </div>

        {/* Season picker + actions */}
        <div className="flex items-center gap-2 mb-4 flex-wrap">
          <span className="text-xs text-[var(--color-text-tertiary)]">
            Season:
          </span>
          {seasons.map((s) => (
            <Button
              key={s}
              variant={activeSeason === s ? "default" : "outline"}
              size="sm"
              className="text-xs"
              onClick={() => setSelectedSeason(s)}
            >
              {s}
            </Button>
          ))}
          <div className="flex-1" />
          <Button variant="ghost" size="sm" onClick={handleTranscodeSeason}>
            <Wand2 size={14} />
            Transcode Season
          </Button>
          <Button variant="ghost" size="sm" onClick={handleFetchMetadata}>
            <RotateCcw size={14} />
            Fetch Metadata
          </Button>
        </div>

        {/* Episodes for selected season */}
        <div className="flex flex-col gap-3">
          {seasonEpisodes.map((ep) => (
            <MediaCard
              key={ep.id}
              entry={ep}
              isPlaying={playing?.id === ep.id}
              onPlay={setPlaying}
              onDelete={handleDelete}
            />
          ))}
        </div>

        {playing && (
          <VideoPlayer
            mediaId={playing.id}
            title={
              playing.episode
                ? `S${String(playing.season ?? 0).padStart(2, "0")}E${String(playing.episode).padStart(2, "0")} - ${playing.episode_title ?? playing.title}`
                : playing.title
            }
            startPosition={playing.play_position ?? undefined}
            onClose={() => setPlaying(null)}
          />
        )}
      </>
    );
  }

  // Main library: show cards (clickable) + movie cards
  return (
    <>
      {continueWatching.length > 0 && (
        <div className="mb-6">
          <h2 className="text-sm font-semibold text-[var(--color-text-secondary)] mb-3 flex items-center gap-2">
            <Play size={14} />
            Continue Watching
          </h2>
          <div className="flex gap-3 overflow-x-auto pb-2">
            {continueWatching.map((entry) => (
              <button
                key={entry.id}
                type="button"
                className="shrink-0 w-32 group cursor-pointer text-left"
                onClick={() => setPlaying(entry)}
              >
                <div className="relative aspect-[2/3] rounded-lg overflow-hidden bg-gradient-to-br from-blue-900/40 to-cyan-900/30">
                  {entry.poster_url ? (
                    <img
                      src={entry.poster_url}
                      alt={entry.title}
                      className="w-full h-full object-cover"
                    />
                  ) : (
                    <div className="w-full h-full flex items-center justify-center">
                      <span className="text-3xl font-bold text-white/20">
                        {entry.title.charAt(0).toUpperCase()}
                      </span>
                    </div>
                  )}
                  <div className="absolute inset-0 bg-black/0 group-hover:bg-black/40 transition-colors flex items-center justify-center">
                    <Play
                      size={28}
                      className="text-white/0 group-hover:text-white/90 transition-colors fill-current"
                    />
                  </div>
                  <div className="absolute bottom-0 left-0 right-0 h-1 bg-black/50">
                    <div
                      className="h-full bg-red-500"
                      style={{
                        width: `${progressPercent(entry)}%`,
                      }}
                    />
                  </div>
                </div>
                <p className="text-xs text-[var(--color-text-primary)] mt-1.5 truncate">
                  {entry.episode
                    ? `S${String(entry.season ?? 0).padStart(2, "0")}E${String(entry.episode).padStart(2, "0")}`
                    : entry.title}
                </p>
                <p className="text-xs text-[var(--color-text-tertiary)]">
                  {formatPlaybackTime(entry.play_position ?? 0)} /{" "}
                  {formatPlaybackTime(entry.duration ?? 0)}
                </p>
              </button>
            ))}
          </div>
        </div>
      )}

      {shows.length > 0 && (
        <div className="mb-6">
          <h2 className="text-sm font-semibold text-[var(--color-text-secondary)] mb-3 flex items-center gap-2">
            <Tv size={14} />
            TV Shows
          </h2>
          <div className="flex flex-col gap-3">
            {shows.map((group) => (
              <ShowCard
                key={group.group_id ?? "ungrouped"}
                group={group}
                playingId={playing?.id ?? null}
                onPlay={setPlaying}
                onSelect={() => selectShow(group)}
              />
            ))}
          </div>
        </div>
      )}

      {movies.length > 0 && (
        <div>
          <h2 className="text-sm font-semibold text-[var(--color-text-secondary)] mb-3 flex items-center gap-2">
            <Film size={14} />
            Movies
          </h2>
          <div className="flex flex-col gap-3">
            {movies.map((entry) => (
              <MediaCard
                key={entry.id}
                entry={entry}
                isPlaying={playing?.id === entry.id}
                onPlay={setPlaying}
                onDelete={handleDelete}
              />
            ))}
          </div>
        </div>
      )}

      {playing && (
        <VideoPlayer
          mediaId={playing.id}
          title={playing.episode_title ?? playing.title}
          startPosition={playing.play_position ?? undefined}
          onClose={() => setPlaying(null)}
        />
      )}
    </>
  );
}
