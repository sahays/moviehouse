import { LayoutGrid, Download, Settings } from "lucide-react";

type View = "library" | "downloads" | "settings";

interface BottomNavProps {
  currentView: View;
  onViewChange: (view: View) => void;
}

const navItems: { id: View; label: string; Icon: typeof LayoutGrid }[] = [
  { id: "library", label: "Library", Icon: LayoutGrid },
  { id: "downloads", label: "Downloads", Icon: Download },
  { id: "settings", label: "Settings", Icon: Settings },
];

export function BottomNav({ currentView, onViewChange }: BottomNavProps) {
  return (
    <nav className="fixed bottom-0 left-0 right-0 bg-[var(--color-bg-secondary)] border-t border-[var(--color-border)] flex items-center justify-around py-2 pb-[max(0.5rem,env(safe-area-inset-bottom))] md:hidden z-40">
      {navItems.map(({ id, label, Icon }) => {
        const active = currentView === id;
        return (
          <button
            key={id}
            onClick={() => onViewChange(id)}
            className={`flex flex-col items-center gap-0.5 px-4 py-1 rounded-lg transition-colors ${
              active
                ? "text-blue-400"
                : "text-[var(--color-text-tertiary)] hover:text-[var(--color-text-secondary)]"
            }`}
          >
            <Icon size={20} strokeWidth={active ? 2 : 1.5} />
            <span className="text-[10px] font-medium">{label}</span>
          </button>
        );
      })}
    </nav>
  );
}
