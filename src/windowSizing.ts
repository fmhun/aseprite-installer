import { invoke } from "@tauri-apps/api/core";
import { useEffect } from "react";

export const DEFAULT_WINDOW_HEIGHT = 680;
const MIN_WINDOW_HEIGHT = 420;
const WINDOW_MARGIN = 24;

export function calculateWindowHeight(
  contentHeight: number,
  availableHeight: number,
): number {
  const maximum = Math.max(MIN_WINDOW_HEIGHT, Math.floor(availableHeight - WINDOW_MARGIN));
  return Math.min(maximum, Math.max(MIN_WINDOW_HEIGHT, Math.ceil(contentHeight)));
}

async function setWindowHeight(): Promise<void> {
  if (!("__TAURI_INTERNALS__" in window)) return;

  const availableHeight = window.screen?.availHeight || 900;
  const height = calculateWindowHeight(DEFAULT_WINDOW_HEIGHT, availableHeight);
  await invoke("resize_window", { height });
}

export function useFixedWindowHeight(): void {
  useEffect(() => {
    void setWindowHeight();
  }, []);
}
