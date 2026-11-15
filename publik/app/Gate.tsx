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
import { SplitLayout } from "@/fork/onboarding/Layout";
import { Scene } from "@/fork/onboarding/scenes/Scene";
import "@/fork/onboarding/onboarding.css";

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

  const [main, ...others] = screen?.buttons ?? [];
  // Onboarding's Welcome layout (Design W1): the words and the black action
  // on the left, the live dictation demo on the right.
  return (
    <main className="relative h-screen bg-(--wb-panel) text-(--wb-ink) select-none">
      {/* The app window's title strip: drag it like the Hub's. */}
      <div
        data-tauri-drag-region
        className="absolute inset-x-0 top-0 z-10 h-[46px]"
        aria-hidden="true"
      />
      {screen && (
        <SplitLayout
          title={screen.title}
          subtitle={screen.body}
          aside={
            <div className="w-full max-w-[460px]">
              <Scene choice="email" />
            </div>
          }
          action={
            main && (
              <Button
                variant="primary"
                size="lg"
                block
                onClick={() => press(main[0])}
              >
                {main[1]}
              </Button>
            )
          }
        >
          <div className="flex flex-col items-start gap-3">
            {others.map(([id, label]) => (
              <Button
                key={id}
                variant="secondary"
                size="lg"
                onClick={() => press(id)}
              >
                {label}
              </Button>
            ))}
            {screen.note && (
              <p
                role="status"
                className="m-0 text-[15px] leading-5 text-(--wb-ink-2)"
              >
                {screen.note}
              </p>
            )}
          </div>
        </SplitLayout>
      )}
    </main>
  );
}
