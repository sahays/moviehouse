import { CheckCheck, MoreVertical, Play, Trash2 } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { MediaEntry } from "../../types";

interface WatchedRailProps {
  entries: MediaEntry[];
  onPlay: (entry: MediaEntry) => void;
  onCleanup: (entry: MediaEntry) => void;
}

/**
 * Horizontal rail of finished titles, each with an overflow menu to delete the
 * files and retire the entry. Distinct from Settings → "Clean Up Sources",
 * which reclaims redundant source files across the whole library and keeps both
 * the transcode and the entry.
 */
export function WatchedRail({ entries, onPlay, onCleanup }: WatchedRailProps) {
  if (entries.length === 0) return null;

  return (
    <section className="mb-6">
      <h2 className="text-sm font-semibold text-[var(--color-text-secondary)] mb-3 flex items-center gap-2">
        <CheckCheck size={14} />
        Already Watched
      </h2>
      <ul className="flex gap-3 overflow-x-auto pb-2 list-none">
        {entries.map((entry) => (
          <li key={entry.id} className="relative shrink-0 w-32 group">
            <button
              type="button"
              className="w-full cursor-pointer text-left"
              onClick={() => onPlay(entry)}
            >
              <div className="relative aspect-[2/3] rounded-lg overflow-hidden bg-gradient-to-br from-blue-900/40 to-cyan-900/30">
                {entry.poster_url ? (
                  <img
                    src={entry.poster_url}
                    alt={entry.title}
                    className="w-full h-full object-cover opacity-60 group-hover:opacity-100 transition-opacity"
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
                <span className="absolute bottom-1 left-1 flex items-center gap-1 rounded bg-black/70 px-1.5 py-0.5 text-[10px] font-medium text-emerald-400">
                  <CheckCheck size={10} />
                  Watched
                </span>
              </div>
              <p className="text-xs text-[var(--color-text-primary)] mt-1.5 truncate">
                {entry.episode
                  ? `S${String(entry.season ?? 0).padStart(2, "0")}E${String(entry.episode).padStart(2, "0")}`
                  : entry.title}
              </p>
            </button>

            {/* Sibling of the play button, not nested inside it — a button
                inside a button is invalid HTML and swallows the click. */}
            <DropdownMenu>
              <DropdownMenuTrigger
                aria-label={`Actions for ${entry.episode_title ?? entry.title}`}
                className="absolute top-1 right-1 rounded bg-black/60 p-1 text-white/70 opacity-0 transition-opacity hover:bg-black/80 hover:text-white focus:opacity-100 group-hover:opacity-100"
              >
                <MoreVertical size={14} />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="min-w-[200px]">
                <DropdownMenuItem onClick={() => onPlay(entry)}>
                  <Play size={14} />
                  Play again
                </DropdownMenuItem>
                <DropdownMenuItem
                  className="text-red-400 focus:text-red-400"
                  onClick={() => onCleanup(entry)}
                >
                  <Trash2 size={14} />
                  Clean up files
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </li>
        ))}
      </ul>
    </section>
  );
}
