import { Button } from "@/components/ui/button";
import { X } from "lucide-react";

interface VideoPlayerProps {
  mediaId: string;
  title: string;
  onClose: () => void;
}

export function VideoPlayer({ mediaId, title, onClose }: VideoPlayerProps) {
  return (
    <div
      className="fixed inset-0 bg-black/90 flex items-center justify-center z-50 p-4"
      onClick={onClose}
    >
      <div className="w-full max-w-5xl" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-lg font-medium text-white">{title}</h2>
          <Button variant="outline" size="sm" onClick={onClose}>
            <X size={16} />
            Close
          </Button>
        </div>
        <video
          controls
          autoPlay
          src={`/api/v1/media/${mediaId}/stream`}
          className="w-full max-h-[80vh] rounded-lg"
        />
      </div>
    </div>
  );
}
