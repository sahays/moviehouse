export interface DataPoint {
  time: number;
  speed: number;
  peers: number;
}

interface SpeedGraphProps {
  data: DataPoint[];
}

function formatTimeLabel(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  if (s === 0) return `${m}m`;
  return `${m}m${s}s`;
}

export function SpeedGraph({ data }: SpeedGraphProps) {
  if (data.length < 2) {
    return (
      <div className="border border-[var(--color-border)] rounded-lg p-2 bg-[var(--color-bg-primary)] my-3">
        <div className="text-center text-xs text-[var(--color-text-tertiary)] py-5">
          Collecting data...
        </div>
      </div>
    );
  }

  const visible = data.slice(-60);

  const W = 600;
  const H = 200;
  const PAD_L = 50;
  const PAD_R = 50;
  const PAD_T = 10;
  const PAD_B = 30;

  const plotW = W - PAD_L - PAD_R;
  const plotH = H - PAD_T - PAD_B;

  const maxSpeed = Math.max(...visible.map((d) => d.speed), 0.01);
  const maxPeers = Math.max(...visible.map((d) => d.peers), 1);

  const firstTime = visible[0].time;
  const lastTime = visible[visible.length - 1].time;
  const timeRange = Math.max(lastTime - firstTime, 1);

  function x(i: number): number {
    const t = visible[i].time - firstTime;
    return PAD_L + (t / timeRange) * plotW;
  }

  function ySpeed(val: number): number {
    return PAD_T + plotH - (val / maxSpeed) * plotH;
  }

  function yPeers(val: number): number {
    return PAD_T + plotH - (val / maxPeers) * plotH;
  }

  const speedPoints = visible
    .map((_, i) => `${x(i)},${ySpeed(visible[i].speed)}`)
    .join(" ");
  const peersPoints = visible
    .map((_, i) => `${x(i)},${yPeers(visible[i].peers)}`)
    .join(" ");

  // Area fill for speed
  const areaPoints = `${x(0)},${PAD_T + plotH} ${speedPoints} ${x(visible.length - 1)},${PAD_T + plotH}`;

  // Grid lines (4 horizontal)
  const gridLines = [];
  for (let i = 1; i <= 4; i++) {
    const gy = PAD_T + (plotH * i) / 5;
    gridLines.push(gy);
  }

  // Time labels (up to 5)
  const timeLabels: { x: number; label: string }[] = [];
  const labelCount = Math.min(5, visible.length);
  for (let i = 0; i < labelCount; i++) {
    const idx = Math.floor((i / (labelCount - 1)) * (visible.length - 1));
    const elapsed = Math.round(visible[idx].time - firstTime);
    timeLabels.push({ x: x(idx), label: formatTimeLabel(elapsed) });
  }

  // Speed axis labels (left)
  const speedLabels: { y: number; label: string }[] = [];
  for (let i = 0; i <= 4; i++) {
    const val = (maxSpeed * (4 - i)) / 4;
    const gy = PAD_T + (plotH * i) / 4;
    speedLabels.push({
      y: gy,
      label: val < 1 ? `${(val * 1024).toFixed(0)}K` : `${val.toFixed(1)}M`,
    });
  }

  // Peer axis labels (right)
  const peerLabels: { y: number; label: string }[] = [];
  for (let i = 0; i <= 4; i++) {
    const val = Math.round((maxPeers * (4 - i)) / 4);
    const gy = PAD_T + (plotH * i) / 4;
    peerLabels.push({ y: gy, label: `${val}` });
  }

  return (
    <div className="border border-[var(--color-border)] rounded-lg p-2 bg-[var(--color-bg-primary)] my-3">
      <svg
        viewBox={`0 0 ${W} ${H}`}
        width="100%"
        preserveAspectRatio="xMidYMid meet"
      >
        {/* Grid lines */}
        {gridLines.map((gy, i) => (
          <line
            key={i}
            x1={PAD_L}
            y1={gy}
            x2={W - PAD_R}
            y2={gy}
            stroke="#222"
            strokeWidth="1"
          />
        ))}

        {/* Area fill */}
        <polygon points={areaPoints} fill="#3b82f6" fillOpacity="0.1" />

        {/* Speed line */}
        <polyline
          points={speedPoints}
          fill="none"
          stroke="#3b82f6"
          strokeWidth="2"
        />

        {/* Peers line (dashed) */}
        <polyline
          points={peersPoints}
          fill="none"
          stroke="#22c55e"
          strokeWidth="1.5"
          strokeDasharray="4 3"
        />

        {/* Left Y axis labels (speed) */}
        {speedLabels.map((sl, i) => (
          <text
            key={i}
            x={PAD_L - 6}
            y={sl.y + 4}
            textAnchor="end"
            fill="#666"
            fontSize="10"
          >
            {sl.label}
          </text>
        ))}

        {/* Right Y axis labels (peers) */}
        {peerLabels.map((pl, i) => (
          <text
            key={i}
            x={W - PAD_R + 6}
            y={pl.y + 4}
            textAnchor="start"
            fill="#666"
            fontSize="10"
          >
            {pl.label}
          </text>
        ))}

        {/* X axis time labels */}
        {timeLabels.map((tl, i) => (
          <text
            key={i}
            x={tl.x}
            y={H - 6}
            textAnchor="middle"
            fill="#666"
            fontSize="10"
          >
            {tl.label}
          </text>
        ))}
      </svg>
      <div className="flex gap-4 justify-center mt-1 text-xs text-[var(--color-text-secondary)]">
        <span className="legend-speed">Speed (MB/s)</span>
        <span className="legend-peers">Peers</span>
      </div>
    </div>
  );
}
