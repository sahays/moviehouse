import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { FolderPicker } from "../FolderPicker";
import type { AppSettings } from "../../types";

interface DownloadSettingsProps {
  settings: AppSettings;
  updateSetting: (
    key: keyof AppSettings,
    value: AppSettings[keyof AppSettings],
  ) => void;
}

export function DownloadSettings({
  settings,
  updateSetting,
}: DownloadSettingsProps) {
  return (
    <>
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
    </>
  );
}
