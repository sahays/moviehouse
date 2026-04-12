import { useState, useMemo } from "react";
import { ChevronLeft, Tv, Film, Wand2, RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { MediaEntry, MediaGroup } from "../types";
import { MediaCard } from "./MediaCard";
import { ShowCard } from "./ShowCard";
import { VideoPlayer } from "./VideoPlayer";
import { formatBytes } from "@/lib/formatters";

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

  const { shows, movies } = useMemo(() => {
    const groupMap = new Map<string, MediaEntry[]>();
    const standalone: MediaEntry[] = [];

    for (const entry of library) {
      if (entry.group_id && entry.show_name) {
        const existing = groupMap.get(entry.group_id) ?? [];
        existing.push(entry);
        groupMap.set(entry.group_id, existing);
      } else {
        standalone.push(entry);
      }
    }

    const showGroups: MediaGroup[] = [];
    for (const [groupId, entries] of groupMap) {
      entries.sort(
        (a, b) =>
          (a.season ?? 0) - (b.season ?? 0) ||
          (a.episode ?? 0) - (b.episode ?? 0),
      );
      const first = entries[0];
      const seasons = new Set(
        entries.map((e) => e.season).filter((s): s is number => s != null),
      );
      showGroups.push({
        group_id: groupId,
        show_name: first.show_name,
        title: first.show_name ?? first.title,
        poster_url: first.poster_url,
        overview: first.overview,
        rating: first.rating,
        is_show: true,
        episode_count: entries.length,
        season_count: seasons.size,
        entries,
      });
    }

    return { shows: showGroups, movies: standalone };
  }, [library]);

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
            onClose={() => setPlaying(null)}
          />
        )}
      </>
    );
  }

  // Main library: show cards (clickable) + movie cards
  return (
    <>
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
          onClose={() => setPlaying(null)}
        />
      )}
    </>
  );
}
