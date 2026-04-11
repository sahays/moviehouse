import { useState, useEffect, useCallback } from "react";
import { ErrorBoundary } from "react-error-boundary";
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
import { SettingsProvider } from "./contexts/SettingsContext";
import type { MediaEntry } from "./types";

type View = "library" | "downloads" | "settings";

function ErrorFallback({ error }: { error: unknown }) {
  const message = error instanceof Error ? error.message : String(error);
  return (
    <div className="p-8 text-center">
      <h2 className="text-lg font-semibold text-red-400">
        Something went wrong
      </h2>
      <p className="text-sm text-[var(--color-text-tertiary)] mt-2">
        {message}
      </p>
      <button
        onClick={() => window.location.reload()}
        className="mt-4 px-4 py-2 bg-blue-600 text-white rounded"
      >
        Reload
      </button>
    </div>
  );
}

function App() {
  const { torrents, addTorrent } = useWebSocket();
  const [view, setView] = useState<View>("library");
  const [library, setLibrary] = useState<MediaEntry[]>([]);
  const { theme, toggleTheme } = useTheme();

  useEffect(() => {
    const fetchLibrary = () => {
      fetch("/api/v1/library")
        .then((r) => r.json())
        .then((data: unknown) => {
          if (Array.isArray(data)) setLibrary(data);
        })
        .catch(() => {});
    };
    fetchLibrary();
    const interval = setInterval(fetchLibrary, 3000);
    return () => clearInterval(interval);
  }, []);

  const refreshLibrary = useCallback(() => {
    fetch("/api/v1/library")
      .then((r) => r.json())
      .then((data: unknown) => {
        if (Array.isArray(data)) setLibrary(data);
      })
      .catch(() => {});
  }, []);

  return (
    <SettingsProvider>
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
              <ErrorBoundary
                fallbackRender={({ error }: { error: unknown }) => (
                  <ErrorFallback error={error} />
                )}
              >
                {view === "library" && (
                  <LibraryView library={library} onRefresh={refreshLibrary} />
                )}
                {view === "downloads" && (
                  <>
                    <AddTorrent onAdded={addTorrent} />
                    <DownloadList torrents={torrents} />
                  </>
                )}
                {view === "settings" && (
                  <SettingsPanel onScanComplete={refreshLibrary} />
                )}
              </ErrorBoundary>
            </main>
          </div>

          {/* Mobile bottom nav */}
          <BottomNav currentView={view} onViewChange={setView} />
        </div>
      </TooltipProvider>
    </SettingsProvider>
  );
}

export default App;
