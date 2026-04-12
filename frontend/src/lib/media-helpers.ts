import type { MediaEntry, TranscodeState } from "../types";

export function getStateLabel(state: TranscodeState): string {
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

export function getStateBadgeClasses(state: TranscodeState): string {
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

export function isPlayable(entry: MediaEntry): boolean {
  if (entry.transcode_state === "Skipped") return true;
  if (
    typeof entry.transcode_state === "object" &&
    "Ready" in entry.transcode_state
  )
    return true;
  if (Object.keys(entry.versions).length > 0) return true;
  return false;
}

export function isActivelyTranscoding(state: TranscodeState): boolean {
  return typeof state === "object" && "Transcoding" in state;
}
