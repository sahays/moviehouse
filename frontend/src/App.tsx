import { useState, useEffect, useCallback } from "react";
import { ErrorBoundary } from "react-error-boundary";
import { BrowserRouter, Routes, Route, Navigate } from "react-router-dom";
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

function AppInner() {
  const { torrents, addTorrent } = useWebSocket();
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
          <Sidebar theme={theme} onToggleTheme={toggleTheme} />

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
                <Routes>
                  <Route
                    path="/"
                    element={
                      <LibraryView
                        library={library}
                        onRefresh={refreshLibrary}
                      />
                    }
                  />
                  <Route
                    path="/downloads"
                    element={
                      <div className="flex flex-col gap-6">
                        <AddTorrent onAdded={addTorrent} />
                        <DownloadList torrents={torrents} />
                      </div>
                    }
                  />
                  <Route
                    path="/settings"
                    element={<SettingsPanel onScanComplete={refreshLibrary} />}
                  />
                  {/* Unknown path → library, rather than a blank page. */}
                  <Route path="*" element={<Navigate to="/" replace />} />
                </Routes>
              </ErrorBoundary>
            </main>
          </div>

          {/* Mobile bottom nav */}
          <BottomNav />
        </div>
      </TooltipProvider>
    </SettingsProvider>
  );
}

export default function App() {
  return (
    <BrowserRouter>
      <AppInner />
    </BrowserRouter>
  );
}
