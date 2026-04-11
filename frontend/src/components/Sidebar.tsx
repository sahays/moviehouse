import { LayoutGrid, Download, Settings, Sun, Moon } from "lucide-react";
import { Logo } from "./Logo";
import { Button } from "@/components/ui/button";

type View = "library" | "downloads" | "settings";

interface SidebarProps {
  currentView: View;
  onViewChange: (view: View) => void;
  theme: "light" | "dark";
  onToggleTheme: () => void;
}

const navItems = [
  { id: "library" as View, label: "Library", Icon: LayoutGrid },
  { id: "downloads" as View, label: "Downloads", Icon: Download },
  { id: "settings" as View, label: "Settings", Icon: Settings },
];

export function Sidebar({
  currentView,
  onViewChange,
  theme,
  onToggleTheme,
}: SidebarProps) {
  return (
    <aside className="hidden md:flex md:flex-col md:w-60 bg-[var(--color-bg-secondary)] border-r border-[var(--color-border)] h-screen sticky top-0">
      <div className="flex items-center gap-2 p-4 border-b border-[var(--color-border)]">
        <Logo size={24} className="text-blue-400" />
        <span className="text-base font-bold text-[var(--color-text-primary)]">
          MovieHouse
        </span>
      </div>
      <nav className="flex-1 p-2 flex flex-col gap-1">
        {navItems.map(({ id, label, Icon }) => (
          <button
            key={id}
            onClick={() => onViewChange(id)}
            className={`flex items-center gap-3 px-3 py-2.5 rounded-lg text-sm transition-colors ${
              currentView === id
                ? "bg-blue-500/10 text-blue-400 font-medium"
                : "text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-tertiary)] hover:text-[var(--color-text-primary)]"
            }`}
          >
            <Icon size={18} />
            <span>{label}</span>
          </button>
        ))}
      </nav>
      <div className="p-3 border-t border-[var(--color-border)]">
        <Button
          variant="ghost"
          size="sm"
          onClick={onToggleTheme}
          className="w-full justify-start gap-2 text-[var(--color-text-tertiary)]"
        >
          {theme === "dark" ? <Sun size={16} /> : <Moon size={16} />}
          <span>{theme === "dark" ? "Light Mode" : "Dark Mode"}</span>
        </Button>
      </div>
    </aside>
  );
}
