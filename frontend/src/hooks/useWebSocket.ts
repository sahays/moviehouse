import { useEffect, useRef, useState, useCallback } from "react";
import type { SessionStatus } from "../types";

interface SnapshotMessage {
  type: "snapshot";
  torrents: SessionStatus[];
}

interface UpdateMessage {
  type: "update";
  id: string;
  status: SessionStatus;
}

interface RemoveMessage {
  type: "remove";
  id: string;
}

type WsMessage = SnapshotMessage | UpdateMessage | RemoveMessage;

export function useWebSocket() {
  const [torrents, setTorrents] = useState<Map<string, SessionStatus>>(
    new Map(),
  );
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const connectRef = useRef<() => void>(() => {});

  useEffect(() => {
    const connect = () => {
      const protocol = location.protocol === "https:" ? "wss:" : "ws:";
      const url = `${protocol}//${location.host}/api/v1/ws`;
      const ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onmessage = (event) => {
        try {
          const msg: WsMessage = JSON.parse(event.data);

          if (msg.type === "snapshot") {
            const next = new Map<string, SessionStatus>();
            for (const t of msg.torrents) {
              next.set(t.id, t);
            }
            setTorrents(next);
          } else if (msg.type === "update") {
            setTorrents((prev) => {
              const next = new Map(prev);
              next.set(msg.id, msg.status);
              return next;
            });
          } else if (msg.type === "remove") {
            setTorrents((prev) => {
              const next = new Map(prev);
              next.delete(msg.id);
              return next;
            });
          }
        } catch {
          // Ignore malformed messages
        }
      };

      ws.onclose = () => {
        wsRef.current = null;
        reconnectTimer.current = setTimeout(() => connectRef.current(), 2000);
      };

      ws.onerror = () => {
        ws.close();
      };
    };

    connectRef.current = connect;
    connect();

    return () => {
      if (reconnectTimer.current) {
        clearTimeout(reconnectTimer.current);
      }
      if (wsRef.current) {
        wsRef.current.close();
      }
    };
  }, []);

  const addTorrent = useCallback((id: string, status: SessionStatus) => {
    setTorrents((prev) => {
      const next = new Map(prev);
      next.set(id, status);
      return next;
    });
  }, []);

  return { torrents, addTorrent };
}
