import { useMemo } from "react";
import type { MediaEntry, MediaGroup } from "../types";

export function useLibraryGroups(library: MediaEntry[]) {
  return useMemo(() => {
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
        title: first.show_name ?? first.title ?? "Unknown",
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
}
