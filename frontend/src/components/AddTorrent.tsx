import { useState, useRef, useCallback } from "react";
import { Upload } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { SessionStatus } from "../types";

interface AddTorrentProps {
  onAdded?: (id: string, status: SessionStatus) => void;
}

export function AddTorrent({ onAdded }: AddTorrentProps) {
  const [magnetUri, setMagnetUri] = useState("");
  const [isDragging, setIsDragging] = useState(false);
  const [status, setStatus] = useState<{
    type: "success" | "error";
    text: string;
  } | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const clearStatus = useCallback(() => {
    setTimeout(() => setStatus(null), 3000);
  }, []);

  const uploadFile = useCallback(
    async (file: File) => {
      const form = new FormData();
      form.append("torrent", file);
      try {
        const res = await fetch("/api/v1/torrents", {
          method: "POST",
          body: form,
        });
        if (!res.ok) {
          const text = await res.text();
          throw new Error(text || res.statusText);
        }
        const data = await res.json();
        if (data.id && data.status && onAdded) {
          onAdded(data.id, data.status);
        }
        setStatus({ type: "success", text: `Added "${file.name}"` });
      } catch (err) {
        setStatus({
          type: "error",
          text: `Failed: ${err instanceof Error ? err.message : "Unknown error"}`,
        });
      }
      clearStatus();
    },
    [clearStatus, onAdded],
  );

  const submitMagnet = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      const uri = magnetUri.trim();
      if (!uri) return;

      const form = new FormData();
      form.append("magnet", uri);
      try {
        const res = await fetch("/api/v1/torrents", {
          method: "POST",
          body: form,
        });
        if (!res.ok) {
          const text = await res.text();
          throw new Error(text || res.statusText);
        }
        setMagnetUri("");
        setStatus({ type: "success", text: "Resolving magnet link..." });
      } catch (err) {
        setStatus({
          type: "error",
          text: `Failed: ${err instanceof Error ? err.message : "Unknown error"}`,
        });
      }
      clearStatus();
    },
    [magnetUri, clearStatus],
  );

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    setIsDragging(true);
  }, []);

  const handleDragLeave = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    setIsDragging(false);
  }, []);

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      setIsDragging(false);
      const files = e.dataTransfer.files;
      if (files.length > 0) {
        uploadFile(files[0]);
      }
    },
    [uploadFile],
  );

  const handleFileSelect = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const files = e.target.files;
      if (files && files.length > 0) {
        uploadFile(files[0]);
      }
      // Reset so same file can be selected again
      e.target.value = "";
    },
    [uploadFile],
  );

  return (
    <div className="bg-[var(--color-bg-secondary)] border border-[var(--color-border)] rounded-xl p-4">
      <h2 className="text-base font-semibold text-[var(--color-text-primary)] mb-3">
        Add Torrent
      </h2>

      <div
        role="button"
        tabIndex={0}
        className={`border-2 border-dashed rounded-lg p-8 text-center cursor-pointer transition-colors ${
          isDragging
            ? "border-blue-500 bg-blue-500/10"
            : "border-[var(--color-border)] hover:border-blue-500/50 hover:bg-blue-500/5"
        }`}
        onDragOver={handleDragOver}
        onDragLeave={handleDragLeave}
        onDrop={handleDrop}
        onClick={() => fileInputRef.current?.click()}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            fileInputRef.current?.click();
          }
        }}
      >
        <input
          ref={fileInputRef}
          type="file"
          accept=".torrent"
          onChange={handleFileSelect}
          style={{ display: "none" }}
        />
        <Upload
          size={40}
          className="mx-auto text-[var(--color-text-tertiary)]"
        />
        <p className="text-sm text-[var(--color-text-tertiary)] mt-2">
          Drop a .torrent file here, or click to browse
        </p>
      </div>

      <form className="flex gap-2 mt-3" onSubmit={submitMagnet}>
        <Input
          type="text"
          className="flex-1"
          placeholder="Paste magnet URI..."
          value={magnetUri}
          onChange={(e) => setMagnetUri(e.target.value)}
        />
        <Button type="submit" disabled={!magnetUri.trim()}>
          <Upload size={16} />
          Add
        </Button>
      </form>

      {status && (
        <div
          className={
            status.type === "success"
              ? "mt-2 text-xs text-emerald-400"
              : "mt-2 text-xs text-red-400"
          }
        >
          {status.text}
        </div>
      )}
    </div>
  );
}
