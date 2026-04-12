import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
} from "react";
import type { AppSettings } from "../types";

interface SettingsContextValue {
  settings: AppSettings | null;
  updateSettings: (updated: AppSettings) => void;
}

const SettingsContext = createContext<SettingsContextValue>({
  settings: null,
  updateSettings: () => {},
});

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

  const updateSettings = useCallback((updated: AppSettings) => {
    setSettings(updated);
  }, []);

  return (
    <SettingsContext.Provider value={{ settings, updateSettings }}>
      {children}
    </SettingsContext.Provider>
  );
}

// eslint-disable-next-line react-refresh/only-export-components -- hook must co-locate with its context
export function useSettings(): SettingsContextValue {
  return useContext(SettingsContext);
}
