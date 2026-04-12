import { useState } from "react";
import { Wand2 } from "lucide-react";
import { Button } from "@/components/ui/button";

interface TranscodeOptionsProps {
  mediaId: string;
  onStarted: () => void;
}

export function TranscodeOptions({
  mediaId,
  onStarted,
}: TranscodeOptionsProps) {
  const [preset, setPreset] = useState("hevc");
  const [submitting, setSubmitting] = useState(false);

  const handleTranscode = async () => {
    setSubmitting(true);
    try {
      await fetch(`/api/v1/library/${mediaId}/transcode`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ preset }),
      });
      onStarted();
    } catch {
      // Network errors handled silently
    }
    setSubmitting(false);
  };

  return (
    <div className="mt-2 space-y-2">
      <div className="flex gap-1">
        <Button
          variant={preset === "hevc" ? "default" : "outline"}
          size="sm"
          className="flex-1 text-xs"
          onClick={() => setPreset("hevc")}
        >
          HEVC (fast)
        </Button>
        <Button
          variant={preset === "h264" ? "default" : "outline"}
          size="sm"
          className="flex-1 text-xs"
          onClick={() => setPreset("h264")}
        >
          H.264
        </Button>
      </div>
      <p className="text-[10px] text-[var(--color-text-tertiary)]">
        {preset === "hevc"
          ? "Remux to MP4 — keeps original quality, very fast."
          : "Re-encode to H.264 MP4 — slower but universal."}
      </p>
      <Button
        className="w-full"
        onClick={handleTranscode}
        disabled={submitting}
      >
        <Wand2 size={16} />
        {submitting ? "Starting..." : "Transcode"}
      </Button>
    </div>
  );
}
