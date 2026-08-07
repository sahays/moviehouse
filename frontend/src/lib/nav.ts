import { LayoutGrid, Download, Settings } from "lucide-react";

/**
 * The app's routes, in nav order. Single source of truth — the sidebar and the
 * mobile bottom nav both render from this, so adding a route in one place
 * updates both.
 */
export const NAV_ITEMS = [
  { to: "/", label: "Library", Icon: LayoutGrid },
  { to: "/downloads", label: "Downloads", Icon: Download },
  { to: "/settings", label: "Settings", Icon: Settings },
] as const;
