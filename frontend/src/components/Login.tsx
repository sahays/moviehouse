import { useEffect, useRef, useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { login } from "../lib/api";
import { Logo } from "./Logo";

export function Login({ onSuccess }: { onSuccess: () => void }) {
  const [code, setCode] = useState("");
  const [error, setError] = useState(false);
  const [busy, setBusy] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(false);
    const ok = await login(code);
    setBusy(false);
    if (ok) {
      onSuccess();
    } else {
      setError(true);
    }
  }

  return (
    <main className="flex min-h-screen items-center justify-center bg-[var(--color-bg-primary)] p-6">
      <form
        onSubmit={handleSubmit}
        className="w-full max-w-sm space-y-4 rounded-lg border border-[var(--color-border)] p-6"
      >
        <div className="flex items-center gap-2">
          <Logo size={24} />
          <h1 className="text-lg font-semibold text-[var(--color-text-primary)]">
            MovieHouse
          </h1>
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="access-code">Access code</Label>
          <Input
            ref={inputRef}
            id="access-code"
            type="password"
            autoComplete="current-password"
            value={code}
            onChange={(e) => setCode(e.target.value)}
          />
        </div>
        {error && (
          <p role="alert" className="text-sm text-destructive">
            Invalid access code.
          </p>
        )}
        <Button
          type="submit"
          disabled={busy || code.length === 0}
          className="w-full"
        >
          {busy ? "Checking…" : "Enter"}
        </Button>
      </form>
    </main>
  );
}
