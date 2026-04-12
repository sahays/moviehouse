import { useState, useEffect, useCallback, useRef } from "react";
import { FolderSearch } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { FolderPicker } from "./FolderPicker";
import { useSettings } from "@/contexts/SettingsContext";
import type { AppSettings, SystemStatus } from "../types";

interface SettingsPanelProps {
  onScanComplete?: () => void;
}

export function SettingsPanel({ onScanComplete }: SettingsPanelProps) {
  const { updateSettings: updateContext } = useSettings();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [systemStatus, setSystemStatus] = useState<SystemStatus | null>(null);
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
    fetch("/api/v1/system/status")
      .then((r) => r.json())
      .then((data: unknown) => {
        if (data && typeof data === "object")
          setSystemStatus(data as SystemStatus);
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

      <div className="flex items-center justify-between py-3">
        <div>
          <Label className="text-sm text-[var(--color-text-secondary)]">
            Lightspeed mode
          </Label>
          <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
            Enable all performance optimizations
          </p>
        </div>
        <Switch
          checked={settings.lightspeed}
          onCheckedChange={(val: boolean) => updateSetting("lightspeed", val)}
        />
      </div>

      <div className="flex items-center justify-between py-3">
        <div>
          <Label className="text-sm text-[var(--color-text-secondary)]">
            Speed limit (MB/s)
          </Label>
          <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
            Limit download speed in MB/s (0 = unlimited)
          </p>
        </div>
        <Input
          type="number"
          className="w-24"
          min="0"
          value={settings.max_download_speed / (1024 * 1024)}
          onChange={(e) =>
            updateSetting(
              "max_download_speed",
              Math.max(0, Number(e.target.value)) * 1024 * 1024,
            )
          }
          placeholder="0 = unlimited"
        />
      </div>

      <div className="flex flex-col gap-2 py-3">
        <div>
          <Label className="text-sm text-[var(--color-text-secondary)]">
            Download folder
          </Label>
          <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
            Where downloaded files are saved
          </p>
        </div>
        <div className="flex gap-2">
          <Input
            type="text"
            className="flex-1"
            value={settings.download_dir}
            onChange={(e) => updateSetting("download_dir", e.target.value)}
          />
          <FolderPicker
            onSelect={(path) => updateSetting("download_dir", path)}
          />
        </div>
      </div>

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

      <div className="flex items-center justify-between py-3">
        <div>
          <Label className="text-sm text-[var(--color-text-secondary)]">
            Auto-transcode
          </Label>
          <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
            Automatically transcode downloads to selected format
          </p>
        </div>
        <Switch
          checked={settings.auto_transcode}
          onCheckedChange={(val: boolean) =>
            updateSetting("auto_transcode", val)
          }
        />
      </div>

      <div className="flex flex-col gap-2 py-3">
        <div>
          <Label className="text-sm text-[var(--color-text-secondary)]">
            Default resolution
          </Label>
          <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
            Resolution for automatic transcoding (Best Compatibility mode)
          </p>
        </div>
        <div className="flex gap-1">
          {["4k", "1080p", "720p", "480p"].map((res) => {
            const presetName = `compat-${res}`;
            const isSelected =
              settings.default_preset === presetName ||
              settings.default_preset === res;
            return (
              <Button
                key={res}
                variant={isSelected ? "default" : "outline"}
                size="sm"
                className="flex-1 text-xs"
                onClick={() => updateSetting("default_preset", presetName)}
              >
                {res.toUpperCase()}
              </Button>
            );
          })}
        </div>
      </div>

      <div className="flex items-center justify-between py-3">
        <div>
          <Label className="text-sm text-[var(--color-text-secondary)]">
            Parallel encoding
          </Label>
          <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
            Split video into chunks and encode in parallel using all CPU cores
          </p>
        </div>
        <Switch
          checked={settings.enable_chunking}
          onCheckedChange={(val: boolean) =>
            updateSetting("enable_chunking", val)
          }
        />
      </div>

      <div className="flex items-center justify-between py-3">
        <div>
          <Label className="text-sm text-[var(--color-text-secondary)]">
            Safari mode
          </Label>
          <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
            Keep H.265/HEVC — fast remux instead of re-encoding. Safari and
            Apple devices only.
          </p>
        </div>
        <Switch
          checked={settings.safari_mode}
          onCheckedChange={(val: boolean) => updateSetting("safari_mode", val)}
        />
      </div>

      <h2 className="text-sm font-semibold text-[var(--color-text-primary)] mt-6 mb-3 uppercase tracking-wider">
        System
      </h2>
      <div className="bg-[var(--color-bg-secondary)] border border-[var(--color-border)] rounded-lg p-3">
        <div className="flex justify-between py-1.5 text-sm">
          <span className="text-[var(--color-text-secondary)]">FFmpeg</span>
          <span
            className={
              systemStatus?.ffmpeg_available
                ? "text-emerald-400"
                : "text-amber-400"
            }
          >
            {systemStatus?.ffmpeg_available ? "Installed" : "Not installed"}
          </span>
        </div>
        {systemStatus?.ffmpeg_version && (
          <div className="flex justify-between py-1.5 text-sm">
            <span className="text-[var(--color-text-secondary)]">Version</span>
            <span className="text-[var(--color-text-tertiary)] text-xs">
              {systemStatus.ffmpeg_version.split(" ").slice(0, 3).join(" ")}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}
