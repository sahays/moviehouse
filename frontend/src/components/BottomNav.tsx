import { NavLink } from "react-router-dom";
import { NAV_ITEMS } from "@/lib/nav";

export function BottomNav() {
  return (
    <nav className="fixed bottom-0 left-0 right-0 bg-[var(--color-bg-secondary)] border-t border-[var(--color-border)] flex items-center justify-around py-2 pb-[max(0.5rem,env(safe-area-inset-bottom))] md:hidden z-40">
      {NAV_ITEMS.map(({ to, label, Icon }) => (
        <NavLink
          key={to}
          to={to}
          end={to === "/"}
          className={({ isActive }) =>
            `flex flex-col items-center gap-0.5 px-4 py-1 rounded-lg transition-colors ${
              isActive
                ? "text-blue-400"
                : "text-[var(--color-text-tertiary)] hover:text-[var(--color-text-secondary)]"
            }`
          }
        >
          {({ isActive }) => (
            <>
              <Icon size={20} strokeWidth={isActive ? 2 : 1.5} />
              <span className="text-[10px] font-medium">{label}</span>
            </>
          )}
        </NavLink>
      ))}
    </nav>
  );
}
