import { useCallback, useEffect, useMemo, useState } from "react";
import { X, ChevronDown, ChevronUp, Download } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { SessionStatus } from "../types";
import { ConfirmDialog } from "./ConfirmDialog";
import { SpeedGraph, type DataPoint } from "./SpeedGraph";
import {
  formatBytes,
  formatSpeed,
  formatTime,
  formatDuration,
} from "@/lib/formatters";

interface DownloadListProps {
  torrents: Map<string, SessionStatus>;
}

function formatElapsed(startedAt: number): string {
  const now = Math.floor(Date.now() / 1000);
  return formatDuration(now - startedAt);
}

function getStateLabel(state: SessionStatus["state"]): string {
  if (typeof state === "string") return state;
  if (state && typeof state === "object" && "Error" in state) return "Error";
  return "Unknown";
}

function getStateBadgeClasses(state: SessionStatus["state"]): string {
  const base = "px-2 py-0.5 rounded text-xs font-medium";
  if (state === "Downloading") return `${base} bg-blue-500/15 text-blue-400`;
  if (state === "Completed")
    return `${base} bg-emerald-500/15 text-emerald-400`;
  if (state === "Cancelled") return `${base} bg-gray-500/15 text-gray-400`;
  if (typeof state === "object" && "Error" in state)
    return `${base} bg-red-500/15 text-red-400`;
  return base;
}

function getErrorMessage(state: SessionStatus["state"]): string | null {
  if (typeof state === "object" && state && "Error" in state) {
    return state.Error;
  }
  return null;
}

interface DeleteConfirmState {
  id: string;
  name: string;
}

function DownloadCard({
  torrent,
  onDelete,
  graphData,
}: {
  torrent: SessionStatus;
  onDelete: (id: string, deleteFiles: boolean) => void;
  graphData: DataPoint[];
}) {
  const progressPercent = (torrent.progress * 100).toFixed(1);
  const errorMsg = getErrorMessage(torrent.state);
  const [expanded, setExpanded] = useState(false);
  const [confirmState, setConfirmState] = useState<DeleteConfirmState | null>(
    null,
  );
  const [, setTick] = useState(0);

  // Live-update elapsed time every second while downloading
  useEffect(() => {
    if (torrent.state !== "Downloading") return;
    const interval = setInterval(() => setTick((t) => t + 1), 1000);
    return () => clearInterval(interval);
  }, [torrent.state]);

  const isCompleted = torrent.state === "Completed";
  const isDownloading = torrent.state === "Downloading";

  return (
    <div className="bg-[var(--color-bg-secondary)] border border-[var(--color-border)] rounded-xl p-4 transition-colors hover:border-[var(--color-border-hover)]">
      <div className="flex items-center justify-between mb-2">
        <div
          className="text-sm font-medium text-[var(--color-text-primary)] truncate flex-1 mr-2"
          title={torrent.name}
        >
          {torrent.name}
        </div>
        <Button
          variant="ghost"
          size="icon-sm"
          onClick={() =>
            setConfirmState({ id: torrent.id, name: torrent.name })
          }
          title="Remove torrent"
          aria-label="Remove torrent"
          className="text-[var(--color-text-tertiary)] hover:text-red-400 hover:bg-red-500/10"
        >
          <X size={16} />
        </Button>
      </div>

      <div className="h-1.5 bg-[var(--color-bg-tertiary)] rounded-full overflow-hidden">
        <div
          className="h-full rounded-full transition-all duration-300"
          style={{
            width: `${torrent.progress * 100}%`,
            background: "linear-gradient(90deg, #3B82F6, #10B981)",
          }}
        />
      </div>

      <div className="flex flex-wrap gap-x-3 gap-y-1 mt-2 text-xs text-[var(--color-text-secondary)]">
        <span className={getStateBadgeClasses(torrent.state)}>
          {getStateLabel(torrent.state)}
        </span>
        <span>{progressPercent}%</span>
        <span>
          {formatBytes(torrent.downloaded_bytes)} /{" "}
          {formatBytes(torrent.total_bytes)}
        </span>
        {isDownloading && (
          <span className="text-cyan-400">
            {formatSpeed(torrent.download_speed)}
          </span>
        )}
        <span>{torrent.peer_count} peers</span>
      </div>

      {/* Compact timing info */}
      <div className="flex flex-wrap gap-3 mt-2 text-xs text-[var(--color-text-tertiary)]">
        <span>Started: {formatTime(torrent.started_at)}</span>
        {isDownloading && (
          <span>Elapsed: {formatElapsed(torrent.started_at)}</span>
        )}
        {isCompleted && torrent.completed_at && (
          <>
            <span>Completed: {formatTime(torrent.completed_at)}</span>
            <span>
              Duration:{" "}
              {formatDuration(torrent.completed_at - torrent.started_at)}
            </span>
          </>
        )}
      </div>

      {/* Expand/collapse button */}
      <Button
        variant="ghost"
        size="sm"
        className="mt-1 text-[var(--color-text-tertiary)]"
        onClick={() => setExpanded(!expanded)}
      >
        {expanded ? (
          <>
            <ChevronUp size={14} />
            Hide details
          </>
        ) : (
          <>
            <ChevronDown size={14} />
            Details
          </>
        )}
      </Button>

      {/* Expanded view */}
      {expanded && (
        <>
          {isDownloading && <SpeedGraph data={graphData} />}

          <div className="flex flex-wrap gap-x-3 gap-y-1 mt-2 text-xs text-[var(--color-text-secondary)]">
            <span>
              {torrent.pieces_done}/{torrent.pieces_total} pieces
            </span>
          </div>

          {isCompleted && (
            <div className="flex flex-wrap gap-3 mt-2 pt-2 border-t border-[var(--color-border)] text-xs text-emerald-400">
              <span>
                Seeding since:{" "}
                {torrent.completed_at ? formatTime(torrent.completed_at) : "--"}
              </span>
              <span>Uploaded: {formatBytes(torrent.uploaded_bytes)}</span>
            </div>
          )}
        </>
      )}

      {errorMsg && (
        <div className="mt-2 text-xs text-red-400 p-2 bg-red-500/10 rounded">
          {errorMsg}
        </div>
      )}

      <ConfirmDialog
        open={confirmState !== null}
        title="Remove torrent"
        message={
          confirmState
            ? `What would you like to do with "${confirmState.name}"?`
            : ""
        }
        confirmLabel="Delete files and remove"
        cancelLabel="Remove from list"
        destructive
        onConfirm={() => {
          if (confirmState) {
            onDelete(confirmState.id, true);
            setConfirmState(null);
          }
        }}
        onCancel={() => {
          if (confirmState) {
            onDelete(confirmState.id, false);
            setConfirmState(null);
          }
        }}
      />
    </div>
  );
}

// Module-level history store (outside React lifecycle)
const historyStore = new Map<string, DataPoint[]>();

function updateHistory(torrents: Map<string, SessionStatus>): void {
  const now = Math.floor(Date.now() / 1000);
  for (const [id, torrent] of torrents) {
    if (torrent.state !== "Downloading") continue;
    const history = historyStore.get(id) ?? [];
    const lastPoint = history[history.length - 1];
    if (
      !lastPoint ||
      lastPoint.speed !== torrent.download_speed ||
      lastPoint.peers !== torrent.peer_count
    ) {
      history.push({
        time: now,
        speed: torrent.download_speed,
        peers: torrent.peer_count,
      });
      if (history.length > 120) {
        history.splice(0, history.length - 120);
      }
      historyStore.set(id, history);
    }
  }
}

export function DownloadList({ torrents }: DownloadListProps) {
  // Update history on each render (side-effect-free, just mutates module-level map)
  useMemo(() => updateHistory(torrents), [torrents]);

  const handleDelete = useCallback(async (id: string, deleteFiles: boolean) => {
    try {
      const url = deleteFiles
        ? `/api/v1/torrents/${id}?delete_files=true`
        : `/api/v1/torrents/${id}`;
      await fetch(url, { method: "DELETE" });
    } catch {
      // Errors are handled by the WebSocket state update
    }
  }, []);

  const sorted = useMemo(
    () =>
      Array.from(torrents.values()).sort((a, b) => {
        // Active downloads first, then completed, then rest
        const order: Record<string, number> = {
          Downloading: 0,
          Completed: 1,
          Cancelled: 2,
        };
        const aOrder = typeof a.state === "string" ? (order[a.state] ?? 3) : 3;
        const bOrder = typeof b.state === "string" ? (order[b.state] ?? 3) : 3;
        if (aOrder !== bOrder) return aOrder - bOrder;
        return a.name.localeCompare(b.name);
      }),
    [torrents],
  );

  if (sorted.length === 0) {
    return (
      <div className="text-center py-16 text-[var(--color-text-tertiary)]">
        <Download size={48} className="mx-auto mb-3" />
        <p>No active downloads</p>
        <p className="text-sm mt-1">
          Add a .torrent file or magnet link above to get started
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      {sorted.map((t) => (
        <DownloadCard
          key={t.id}
          torrent={t}
          onDelete={handleDelete}
          graphData={historyStore.get(t.id) ?? []}
        />
      ))}
    </div>
  );
}
