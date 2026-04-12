import { Switch } from "@/components/ui/switch";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
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
    </>
  );
}
