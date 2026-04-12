import { Play } from "lucide-react";
import type { MediaEntry } from "../../types";

interface MediaCardPosterProps {
  entry: MediaEntry;
  isPlaying: boolean;
  playable: boolean;
  onPlay: (entry: MediaEntry) => void;
}

export function MediaCardPoster({
  entry,
  isPlaying,
  playable,
  onPlay,
}: MediaCardPosterProps) {
  return (
    <div
      role={playable ? "button" : undefined}
      tabIndex={playable ? 0 : undefined}
      className={`relative w-40 sm:w-44 shrink-0 aspect-[2/3] bg-gradient-to-br from-blue-900/40 to-cyan-900/30 flex items-center justify-center overflow-hidden group ${playable ? "cursor-pointer" : ""}`}
      onClick={() => playable && onPlay(entry)}
      onKeyDown={(e) => {
        if (playable && (e.key === "Enter" || e.key === " ")) {
          e.preventDefault();
          onPlay(entry);
        }
      }}
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
      {isPlaying ? (
        <div className="absolute inset-0 bg-black/50 flex flex-col items-center justify-center gap-1">
          <div className="flex items-center gap-1">
            <span className="w-1 h-3 bg-blue-400 rounded-full animate-pulse" />
            <span className="w-1 h-4 bg-blue-400 rounded-full animate-pulse [animation-delay:150ms]" />
            <span className="w-1 h-3 bg-blue-400 rounded-full animate-pulse [animation-delay:300ms]" />
          </div>
          <span className="text-xs font-medium text-blue-300">Now Playing</span>
        </div>
      ) : playable ? (
        <div className="absolute inset-0 bg-black/0 group-hover:bg-black/40 transition-colors flex items-center justify-center">
          <Play
            size={36}
            className="text-white/0 group-hover:text-white/90 transition-colors fill-current"
          />
        </div>
      ) : null}
    </div>
  );
}
