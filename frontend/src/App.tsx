import { useState, useEffect, useCallback } from "react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useWebSocket } from "./hooks/useWebSocket";
import { useTheme } from "./hooks/useTheme";
import { AddTorrent } from "./components/AddTorrent";
import { DownloadList } from "./components/DownloadList";
import { LibraryView } from "./components/LibraryView";
import { SettingsPanel } from "./components/SettingsPanel";
import { Sidebar } from "./components/Sidebar";
import { BottomNav } from "./components/BottomNav";
import { FfmpegBanner } from "./components/FfmpegBanner";
import { Logo } from "./components/Logo";
import type { MediaEntry } from "./types";

type View = "library" | "downloads" | "settings";

function App() {
  const { torrents, addTorrent } = useWebSocket();
  const [view, setView] = useState<View>("library");
  const [library, setLibrary] = useState<MediaEntry[]>([]);
  const { theme, toggleTheme } = useTheme();

  const fetchLibrary = useCallback(() => {
    fetch("/api/v1/library")
      .then((r) => r.json())
      .then((data) => {
        if (Array.isArray(data)) setLibrary(data);
      })
      .catch(() => {});
  }, []);

  useEffect(() => {
    fetchLibrary();
    const interval = setInterval(fetchLibrary, 3000);
    return () => clearInterval(interval);
  }, [fetchLibrary]);

  return (
    <TooltipProvider>
      <div className="flex min-h-screen bg-[var(--color-bg-primary)]">
        {/* Desktop sidebar */}
        <Sidebar
          currentView={view}
          onViewChange={setView}
          theme={theme}
          onToggleTheme={toggleTheme}
        />

        <div className="flex-1 flex flex-col min-h-screen">
          <FfmpegBanner />
          <header className="flex items-center gap-3 px-4 py-3 border-b border-[var(--color-border)] md:hidden">
            <Logo size={24} />
            <h1 className="text-lg font-semibold text-[var(--color-text-primary)]">
              MovieHouse
            </h1>
          </header>
          <main className="flex-1 p-4 max-w-5xl mx-auto w-full pb-20 md:pb-4">
            {view === "library" && (
              <LibraryView library={library} onRefresh={fetchLibrary} />
            )}
            {view === "downloads" && (
              <>
                <AddTorrent onAdded={addTorrent} />
                <DownloadList torrents={torrents} />
              </>
            )}
            {view === "settings" && (
              <SettingsPanel onScanComplete={fetchLibrary} />
            )}
          </main>
        </div>

        {/* Mobile bottom nav */}
        <BottomNav currentView={view} onViewChange={setView} />
      </div>
    </TooltipProvider>
  );
}

export default App;
