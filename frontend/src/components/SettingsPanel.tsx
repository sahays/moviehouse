import { useState, useEffect, useCallback, useRef } from "react";
import { FolderSearch, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { FolderPicker } from "./FolderPicker";
import { useSettings } from "@/contexts/SettingsContext";
import type { AppSettings } from "../types";
import { DownloadSettings } from "./settings/DownloadSettings";
import { TranscodeSettings } from "./settings/TranscodeSettings";
import { SystemInfo } from "./settings/SystemInfo";

function CleanupSection() {
  const [cleaning, setCleaning] = useState(false);
  const [result, setResult] = useState<{
    deleted: number;
    freed_mb: string;
    errors: number;
  } | null>(null);

  const handleCleanup = async () => {
    setCleaning(true);
    setResult(null);
    try {
      const res = await fetch("/api/v1/library/cleanup", { method: "POST" });
      const data = (await res.json()) as {
        deleted: number;
        freed_mb: string;
        errors: number;
      };
      setResult(data);
    } catch {
      setResult({ deleted: 0, freed_mb: "0", errors: 1 });
    }
    setCleaning(false);
  };

  return (
    <div className="py-3">
      <div className="mb-2">
        <p className="text-sm text-[var(--color-text-secondary)]">
          Delete original source files
        </p>
        <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
          Removes source files (MKV, etc.) for media that has been transcoded.
          Transcoded MP4 files are kept.
        </p>
      </div>
      <Button
        variant="outline"
        onClick={handleCleanup}
        disabled={cleaning}
        className="text-red-400 border-red-500/30 hover:bg-red-500/10"
      >
        <Trash2 size={14} />
        {cleaning ? "Cleaning..." : "Clean Up Sources"}
      </Button>
      {result && (
        <div className="text-xs mt-2">
          {result.deleted > 0 ? (
            <span className="text-emerald-400">
              Deleted {result.deleted} files, freed {result.freed_mb} MB
            </span>
          ) : (
            <span className="text-[var(--color-text-tertiary)]">
              No source files to clean up
            </span>
          )}
          {result.errors > 0 && (
            <span className="text-red-400 ml-2">({result.errors} errors)</span>
          )}
        </div>
      )}
    </div>
  );
}

interface SettingsPanelProps {
  onScanComplete?: () => void;
}

export function SettingsPanel({ onScanComplete }: SettingsPanelProps) {
  const { updateSettings: updateContext } = useSettings();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [scanPath, setScanPath] = useState("");
  const [scanResult, setScanResult] = useState<{
    added: number;
    skipped: number;
  } | null>(null);
  const [scanning, setScanning] = useState(false);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    fetch("/api/v1/settings")
      .then((r) => r.json())
      .then((s: unknown) => {
        if (s && typeof s === "object") {
          const settings = s as AppSettings;
          setSettings(settings);
          if (settings.media_scan_dir) setScanPath(settings.media_scan_dir);
        }
      })
      .catch(() => {});
  }, []);

  const updateSetting = useCallback(
    (key: keyof AppSettings, value: AppSettings[keyof AppSettings]) => {
      setSettings((prev) => {
        if (!prev) return prev;
        const next = { ...prev, [key]: value };
        updateContext(next); // Sync to shared context immediately
        if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
        saveTimerRef.current = setTimeout(() => {
          fetch("/api/v1/settings", {
            method: "PUT",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(next),
          });
        }, 500);
        return next;
      });
    },
    [updateContext],
  );

  const handleScan = async () => {
    if (!scanPath.trim()) return;
    setScanning(true);
    setScanResult(null);
    try {
      const res = await fetch("/api/v1/library/scan", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path: scanPath }),
      });
      const data = await res.json();
      setScanResult(data);
      onScanComplete?.();
    } catch {
      setScanResult({ added: 0, skipped: 0 });
    }
    setScanning(false);
  };

  if (!settings)
    return (
      <div className="text-[var(--color-text-tertiary)] text-center py-10 text-sm">
        Loading settings...
      </div>
    );

  return (
    <div>
      <h2 className="text-sm font-semibold text-[var(--color-text-primary)] mb-3 uppercase tracking-wider">
        Download
      </h2>

      <DownloadSettings settings={settings} updateSetting={updateSetting} />

      <h2 className="text-sm font-semibold text-[var(--color-text-primary)] mt-6 mb-3 uppercase tracking-wider">
        Media Scan
      </h2>

      <div className="flex flex-col gap-2 py-3">
        <div>
          <Label className="text-sm text-[var(--color-text-secondary)]">
            Scan folder for existing media
          </Label>
          <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
            Import existing media files from this folder
          </p>
        </div>
        <div className="flex gap-2">
          <Input
            type="text"
            className="flex-1"
            value={scanPath}
            onChange={(e) => {
              setScanPath(e.target.value);
              updateSetting("media_scan_dir", e.target.value || null);
            }}
            placeholder="/path/to/movies"
          />
          <FolderPicker
            onSelect={(path) => {
              setScanPath(path);
              updateSetting("media_scan_dir", path);
            }}
          />
          <Button onClick={handleScan} disabled={scanning || !scanPath.trim()}>
            <FolderSearch size={16} />
            {scanning ? "Scanning..." : "Scan"}
          </Button>
        </div>
        {scanResult && (
          <div className="text-xs text-emerald-400 mt-1">
            Added {scanResult.added} files, {scanResult.skipped} already in
            library
          </div>
        )}
      </div>

      <h2 className="text-sm font-semibold text-[var(--color-text-primary)] mt-6 mb-3 uppercase tracking-wider">
        Transcoding
      </h2>

      <TranscodeSettings settings={settings} updateSetting={updateSetting} />

      <h2 className="text-sm font-semibold text-[var(--color-text-primary)] mt-6 mb-3 uppercase tracking-wider">
        Cleanup
      </h2>

      <CleanupSection />

      <h2 className="text-sm font-semibold text-[var(--color-text-primary)] mt-6 mb-3 uppercase tracking-wider">
        System
      </h2>

      <SystemInfo />
    </div>
  );
}
