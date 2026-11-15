// SPDX-License-Identifier: GPL-3.0-or-later
/**
 * "Sign in with publik". The Rust gate (publik/rust/src/gate.rs)
 * decides what to show and what each button does; this page shows the
 * current screen and reports presses. The words are publik's own, in
 * English, as publik's gate has them.
 */
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/fork/ui/Button";

export interface GateScreen {
  id: string;
  title: string;
  body: string;
  note: string | null;
  /** `[id, label]`; the first is the main action. */
  buttons: [string, string][];
}

export function Gate() {
  const [screen, setScreen] = useState<GateScreen | null>(null);

  useEffect(() => {
    let alive = true;
    const read = () =>
      invoke<GateScreen | null>("publik_gate_screen")
        .then((s) => {
          if (alive) setScreen(s);
        })
        .catch(() => {});
    void read();
    const timer = window.setInterval(read, 300);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  const press = (id: string) => {
    void invoke("publik_gate_press", { id }).catch(() => {});
  };

  return (
    <main className="relative flex h-screen flex-col items-center justify-center bg-(--wb-surface) px-8 text-(--wb-ink) select-none">
      {/* The app window's title strip: drag it like the Hub's. */}
      <div
        data-tauri-drag-region
        className="absolute inset-x-0 top-0 h-[46px]"
        aria-hidden="true"
      />
      {screen && (
        <div className="flex w-full max-w-[420px] flex-col items-center gap-4 text-center">
          <h1 className="text-[24px] font-semibold">{screen.title}</h1>
          <p className="text-[15px] leading-relaxed text-(--wb-ink-2)">
            {screen.body}
          </p>
          <div className="flex flex-wrap justify-center gap-2 pt-2">
            {screen.buttons.map(([id, label], i) => (
              <Button
                key={id}
                variant={i === 0 ? "primary" : "secondary"}
                size="lg"
                onClick={() => press(id)}
              >
                {label}
              </Button>
            ))}
          </div>
          {screen.note && (
            <p role="status" className="text-[13px] text-(--wb-ink-3)">
              {screen.note}
            </p>
          )}
        </div>
      )}
    </main>
  );
}
