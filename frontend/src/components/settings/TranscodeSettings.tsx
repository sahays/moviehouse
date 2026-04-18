import { useState } from "react";
import { Switch } from "@/components/ui/switch";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { FolderPicker } from "../FolderPicker";
import { FolderOutput } from "lucide-react";
import type { AppSettings } from "../../types";

interface TranscodeSettingsProps {
  settings: AppSettings;
  updateSetting: (
    key: keyof AppSettings,
    value: AppSettings[keyof AppSettings],
  ) => void;
}

export function TranscodeSettings({
  settings,
  updateSetting,
}: TranscodeSettingsProps) {
  const [migrating, setMigrating] = useState(false);
  const [migratePath, setMigratePath] = useState(settings.transcode_dir ?? "");
  const [migrateResult, setMigrateResult] = useState<{
    moved: number;
    moved_mb: string;
    errors: number;
  } | null>(null);

  const handleMigrate = async () => {
    if (!migratePath.trim()) return;
    setMigrating(true);
    setMigrateResult(null);
    try {
      const res = await fetch("/api/v1/library/migrate", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path: migratePath }),
      });
      const data = (await res.json()) as {
        moved: number;
        moved_mb: string;
        errors: number;
      };
      setMigrateResult(data);
      updateSetting("transcode_dir", migratePath);
    } catch {
      setMigrateResult({ moved: 0, moved_mb: "0", errors: 1 });
    }
    setMigrating(false);
  };

  return (
    <>
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

      <div className="flex items-center justify-between py-3">
        <div>
          <Label className="text-sm text-[var(--color-text-secondary)]">
            Default encoding
          </Label>
          <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
            Used for auto-transcode and batch transcode
          </p>
        </div>
        <div className="flex gap-1">
          <Button
            variant={settings.default_preset === "hevc" ? "default" : "outline"}
            size="sm"
            className="text-xs"
            onClick={() => updateSetting("default_preset", "hevc")}
          >
            HEVC
          </Button>
          <Button
            variant={settings.default_preset === "h264" ? "default" : "outline"}
            size="sm"
            className="text-xs"
            onClick={() => updateSetting("default_preset", "h264")}
          >
            H.264
          </Button>
        </div>
      </div>

      <div className="flex flex-col gap-2 py-3">
        <div>
          <Label className="text-sm text-[var(--color-text-secondary)]">
            Media storage path
          </Label>
          <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
            Move all transcoded files and subtitles to a new location (e.g.
            flash drive)
          </p>
        </div>
        <div className="flex gap-2">
          <Input
            type="text"
            className="flex-1"
            value={migratePath}
            onChange={(e) => setMigratePath(e.target.value)}
            placeholder={settings.transcode_dir}
          />
          <FolderPicker onSelect={(path) => setMigratePath(path)} />
          <Button
            onClick={handleMigrate}
            disabled={migrating || !migratePath.trim()}
          >
            <FolderOutput size={16} />
            {migrating ? "Saving..." : "Save"}
          </Button>
        </div>
        {migrateResult && (
          <div className="text-xs mt-1">
            {migrateResult.moved > 0 ? (
              <span className="text-emerald-400">
                Moved {migrateResult.moved} files ({migrateResult.moved_mb} MB)
                to new location
              </span>
            ) : (
              <span className="text-emerald-400">Storage path updated</span>
            )}
            {migrateResult.errors > 0 && (
              <span className="text-red-400 ml-2">
                ({migrateResult.errors} errors)
              </span>
            )}
          </div>
        )}
      </div>
    </>
  );
}
