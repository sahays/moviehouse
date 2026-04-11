export interface SessionStatus {
  id: string;
  name: string;
  info_hash: string;
  state: "Downloading" | "Completed" | "Cancelled" | { Error: string };
  total_bytes: number;
  downloaded_bytes: number;
  pieces_done: number;
  pieces_total: number;
  peer_count: number;
  download_speed: number;
  progress: number;
  started_at: number;
  completed_at: number | null;
  uploaded_bytes: number;
}

export type TranscodeState =
  | "Pending"
  | { Transcoding: { progress_percent: number; encoder: string } }
  | { Ready: { output_path: string } }
  | { Failed: { error: string } }
  | "Skipped"
  | "Unavailable";

export interface MediaEntry {
  id: string;
  title: string;
  year: number | null;
  media_type: "Movie" | "Show" | "Unknown";
  original_path: string;
  media_file: string;
  transcoded_path: string | null;
  transcode_state: TranscodeState;
  transcode_started_at: number | null;
  download_id: string;
  added_at: number;
  file_size: number;
  poster_url: string | null;
  overview: string | null;
  rating: number | null;
  cast: string[];
  director: string | null;
  video_codec: string | null;
  audio_codec: string | null;
  versions: Record<string, string>;
}

export interface SystemStatus {
  ffmpeg_available: boolean;
  ffmpeg_version: string | null;
}

export interface AppSettings {
  lightspeed: boolean;
  max_download_speed: number;
  download_dir: string;
  media_scan_dir: string | null;
  auto_transcode: boolean;
  default_preset: string;
  default_container: string;
  enable_chunking: boolean;
}

export interface TranscodePreset {
  name: string;
  label: string;
}
