import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { X, Upload, Subtitles } from "lucide-react";

interface SubtitleInfo {
  index: number;
  label: string;
  language: string | null;
  format: string;
}

interface VideoPlayerProps {
  mediaId: string;
  title: string;
  startPosition?: number;
  onClose: () => void;
}

const PROGRESS_INTERVAL = 10; // seconds between progress saves

export function VideoPlayer({
  mediaId,
  title,
  startPosition,
  onClose,
}: VideoPlayerProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const lastSentRef = useRef<number>(0);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [subtitles, setSubtitles] = useState<SubtitleInfo[]>([]);
  const [uploading, setUploading] = useState(false);

  const fetchSubtitles = useCallback(() => {
    fetch(`/api/v1/media/${mediaId}/subtitles`)
      .then((r) => r.json())
      .then((data: SubtitleInfo[]) => {
        if (Array.isArray(data)) setSubtitles(data);
      })
      .catch(() => {});
  }, [mediaId]);

  useEffect(() => {
    fetchSubtitles();
  }, [fetchSubtitles]);

  const handleUpload = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    setUploading(true);
    const form = new FormData();
    form.append("file", file);
    try {
      await fetch(`/api/v1/media/${mediaId}/subtitles`, {
        method: "POST",
        body: form,
      });
      fetchSubtitles();
    } catch {
      // ignore
    }
    setUploading(false);
    if (fileInputRef.current) fileInputRef.current.value = "";
  };

  const sendProgress = useCallback(
    (position: number, duration: number) => {
      if (duration <= 0) return;
      fetch(`/api/v1/media/${mediaId}/progress`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ position, duration }),
        keepalive: true,
      }).catch(() => {});
    },
    [mediaId],
  );

  // Set up progress tracking and resume
  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;

    const handleLoadedMetadata = () => {
      if (startPosition && startPosition > 0) {
        video.currentTime = startPosition;
      }
    };

    const handleTimeUpdate = () => {
      const now = video.currentTime;
      if (Math.abs(now - lastSentRef.current) >= PROGRESS_INTERVAL) {
        lastSentRef.current = now;
        sendProgress(now, video.duration);
      }
    };

    const handlePause = () => {
      sendProgress(video.currentTime, video.duration);
      lastSentRef.current = video.currentTime;
    };

    video.addEventListener("loadedmetadata", handleLoadedMetadata);
    video.addEventListener("timeupdate", handleTimeUpdate);
    video.addEventListener("pause", handlePause);

    return () => {
      video.removeEventListener("loadedmetadata", handleLoadedMetadata);
      video.removeEventListener("timeupdate", handleTimeUpdate);
      video.removeEventListener("pause", handlePause);
      // Save final progress on unmount
      if (video.currentTime > 0 && video.duration > 0) {
        sendProgress(video.currentTime, video.duration);
      }
    };
  }, [startPosition, sendProgress]);

  return (
    // eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions -- modal overlay dismiss
    <div
      role="dialog"
      aria-label={`Playing ${title}`}
      className="fixed inset-0 bg-black/90 flex items-center justify-center z-50 p-4"
      onClick={onClose}
      onKeyDown={(e) => {
        if (e.key === "Escape") onClose();
      }}
    >
      {/* eslint-disable-next-line jsx-a11y/click-events-have-key-events, jsx-a11y/no-static-element-interactions */}
      <div className="w-full max-w-5xl" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-lg font-medium text-white">{title}</h2>
          <div className="flex items-center gap-2">
            {subtitles.length > 0 ? (
              <span className="flex items-center gap-1 text-xs text-purple-400">
                <Subtitles size={14} />
                {subtitles.length} sub{subtitles.length > 1 ? "s" : ""}
              </span>
            ) : (
              <>
                <input
                  ref={fileInputRef}
                  type="file"
                  accept=".srt,.vtt,.ass,.ssa"
                  className="hidden"
                  onChange={handleUpload}
                />
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => fileInputRef.current?.click()}
                  disabled={uploading}
                >
                  <Upload size={14} />
                  {uploading ? "Uploading..." : "Add Subtitles"}
                </Button>
              </>
            )}
            <Button variant="outline" size="sm" onClick={onClose}>
              <X size={16} />
              Close
            </Button>
          </div>
        </div>
        <video
          ref={videoRef}
          controls
          autoPlay
          src={`/api/v1/media/${mediaId}/stream`}
          className="w-full max-h-[80vh] rounded-lg"
          crossOrigin="anonymous"
        >
          {subtitles.map((sub, i) => (
            <track
              key={sub.index}
              kind="subtitles"
              src={`/api/v1/media/${mediaId}/subtitles/${sub.index}`}
              label={sub.label}
              srcLang={sub.language ?? undefined}
              default={i === 0}
            />
          ))}
        </video>
      </div>
    </div>
  );
}
