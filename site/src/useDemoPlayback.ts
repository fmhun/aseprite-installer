import { useEffect, useState, type RefObject } from "react";
import { DEMO_DURATION, getDemoFrame } from "./demo";

const MOTION_QUERY = "(prefers-reduced-motion: reduce)";

function initialReducedMotion(): boolean {
  return typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia(MOTION_QUERY).matches;
}

export function useDemoPlayback(containerRef: RefObject<HTMLElement | null>) {
  const [elapsed, setElapsed] = useState(0);
  const [inViewport, setInViewport] = useState(
    () => typeof window === "undefined" || !("IntersectionObserver" in window),
  );
  const [pageVisible, setPageVisible] = useState(
    () => typeof document === "undefined" || document.visibilityState !== "hidden",
  );
  const [reducedMotion, setReducedMotion] = useState(initialReducedMotion);

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const media = window.matchMedia(MOTION_QUERY);
    const updatePreference = () => setReducedMotion(media.matches);
    updatePreference();
    media.addEventListener("change", updatePreference);
    return () => media.removeEventListener("change", updatePreference);
  }, []);

  useEffect(() => {
    const updateVisibility = () => setPageVisible(document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", updateVisibility);
    return () => document.removeEventListener("visibilitychange", updateVisibility);
  }, []);

  useEffect(() => {
    const element = containerRef.current;
    if (!element || !("IntersectionObserver" in window)) {
      setInViewport(true);
      return;
    }

    const observer = new IntersectionObserver(
      ([entry]) => setInViewport(Boolean(entry?.isIntersecting)),
      { threshold: 0.2 },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [containerRef]);

  useEffect(() => {
    if (reducedMotion) {
      setElapsed(DEMO_DURATION - 1);
      return;
    }
    if (!inViewport || !pageVisible) return;

    let previous = performance.now();
    const timer = window.setInterval(() => {
      const now = performance.now();
      const delta = Math.max(0, Math.min(250, now - previous));
      previous = now;
      setElapsed((current) => (current + delta) % DEMO_DURATION);
    }, 80);

    return () => window.clearInterval(timer);
  }, [inViewport, pageVisible, reducedMotion]);

  return {
    ...getDemoFrame(elapsed, reducedMotion),
    isPlaying: !reducedMotion && inViewport && pageVisible,
    reducedMotion,
  };
}
