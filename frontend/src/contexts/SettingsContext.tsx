import { createContext, useContext, useEffect, useState } from "react";
import type { AppSettings } from "../types";

const SettingsContext = createContext<AppSettings | null>(null);

export function SettingsProvider({ children }: { children: React.ReactNode }) {
  const [settings, setSettings] = useState<AppSettings | null>(null);

  useEffect(() => {
    fetch("/api/v1/settings")
      .then((r) => r.json())
      .then((data: unknown) => {
        if (data && typeof data === "object") setSettings(data as AppSettings);
      })
      .catch(() => {});
  }, []);

  return (
    <SettingsContext.Provider value={settings}>
      {children}
    </SettingsContext.Provider>
  );
}

// eslint-disable-next-line react-refresh/only-export-components -- hook must co-locate with its context
export function useSettings(): AppSettings | null {
  return useContext(SettingsContext);
}
