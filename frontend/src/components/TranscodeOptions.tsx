import { useState } from "react";
import { Wand2 } from "lucide-react";
import { Button } from "@/components/ui/button";

interface TranscodeOptionsProps {
  mediaId: string;
  videoCodec: string | null;
  onStarted: () => void;
}

type Mode = "compatibility" | "quality";

const resolutions = [
  { value: "4k", label: "4K" },
  { value: "1080p", label: "1080p" },
  { value: "720p", label: "720p" },
  { value: "480p", label: "480p" },
];

export function TranscodeOptions({
  mediaId,
  videoCodec,
  onStarted,
}: TranscodeOptionsProps) {
  // Only allow "Best Quality" (remux) for H.264 sources — HEVC doesn't play reliably in browsers
  const canRemux = videoCodec === "h264";

  const [mode, setMode] = useState<Mode>("compatibility");
  const [resolution, setResolution] = useState("1080p");
  const [submitting, setSubmitting] = useState(false);

  const presetName =
    mode === "quality" ? "quality-original" : `compat-${resolution}`;

  const handleTranscode = async () => {
    setSubmitting(true);
    try {
      await fetch(`/api/v1/library/${mediaId}/transcode`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ preset: presetName, container: "mp4" }),
      });
      onStarted();
    } catch {
      // Network errors handled silently
    }
    setSubmitting(false);
  };

  return (
    <div className="mt-2 space-y-2">
      {/* Mode toggle — only show if remux is viable */}
      {canRemux && (
        <div className="flex gap-1">
          <Button
            variant={mode === "compatibility" ? "default" : "outline"}
            size="sm"
            className="flex-1 text-xs"
            onClick={() => setMode("compatibility")}
          >
            Best Compatibility
          </Button>
          <Button
            variant={mode === "quality" ? "default" : "outline"}
            size="sm"
            className="flex-1 text-xs"
            onClick={() => setMode("quality")}
          >
            Best Quality
          </Button>
        </div>
      )}

      <p className="text-[10px] text-[var(--color-text-tertiary)]">
        {mode === "quality" && canRemux
          ? "Copies original H.264 streams. Instant, plays everywhere."
          : "Re-encodes to H.264. Plays on all browsers and devices."}
      </p>

      {/* Resolution picker — only for compatibility mode */}
      {mode === "compatibility" && (
        <div className="flex gap-1">
          {resolutions.map((r) => (
            <Button
              key={r.value}
              variant={resolution === r.value ? "default" : "outline"}
              size="sm"
              className="flex-1 text-xs"
              onClick={() => setResolution(r.value)}
            >
              {r.label}
            </Button>
          ))}
        </div>
      )}

      <Button
        className="w-full"
        onClick={handleTranscode}
        disabled={submitting}
      >
        <Wand2 size={16} />
        {submitting
          ? "Starting..."
          : mode === "quality" && canRemux
            ? "Remux Now"
            : "Transcode"}
      </Button>
    </div>
  );
}
