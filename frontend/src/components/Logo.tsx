import { Clapperboard } from "lucide-react";

interface LogoProps {
  size?: number;
  className?: string;
}

export function Logo({ size = 24, className = "" }: LogoProps) {
  return <Clapperboard size={size} className={className} />;
}
