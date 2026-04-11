import { useState } from "react";
import type { MediaEntry } from "../types";
import { MediaCard } from "./MediaCard";
import { VideoPlayer } from "./VideoPlayer";

interface LibraryViewProps {
  library: MediaEntry[];
  onRefresh: () => void;
}

export function LibraryView({ library, onRefresh }: LibraryViewProps) {
  const [playing, setPlaying] = useState<MediaEntry | null>(null);

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

  return (
    <>
      <div className="flex flex-col gap-4">
        {library.map((entry) => (
          <MediaCard
            key={entry.id}
            entry={entry}
            onPlay={setPlaying}
            onDelete={handleDelete}
          />
        ))}
      </div>
      {playing && (
        <VideoPlayer
          mediaId={playing.id}
          title={playing.title}
          onClose={() => setPlaying(null)}
        />
      )}
    </>
  );
}
