import { Sun, Moon } from "lucide-react";
import { NavLink } from "react-router-dom";
import { Logo } from "./Logo";
import { Button } from "@/components/ui/button";
import { NAV_ITEMS } from "@/lib/nav";

interface SidebarProps {
  theme: "light" | "dark";
  onToggleTheme: () => void;
}

export function Sidebar({ theme, onToggleTheme }: SidebarProps) {
  return (
    <aside className="hidden md:flex md:flex-col md:w-60 bg-[var(--color-bg-secondary)] border-r border-[var(--color-border)] h-screen sticky top-0">
      <div className="flex items-center gap-2 p-4 border-b border-[var(--color-border)]">
        <Logo size={24} className="text-blue-400" />
        <span className="text-base font-bold text-[var(--color-text-primary)]">
          MovieHouse
        </span>
      </div>
      <nav className="flex-1 p-2 flex flex-col gap-1">
        {NAV_ITEMS.map(({ to, label, Icon }) => (
          <NavLink
            key={to}
            to={to}
            // `end` so "/" only matches the library, not every route below it.
            end={to === "/"}
            className={({ isActive }) =>
              `flex items-center gap-3 px-3 py-2.5 rounded-lg text-sm transition-colors ${
                isActive
                  ? "bg-blue-500/10 text-blue-400 font-medium"
                  : "text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-tertiary)] hover:text-[var(--color-text-primary)]"
              }`
            }
          >
            <Icon size={18} />
            <span>{label}</span>
          </NavLink>
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
