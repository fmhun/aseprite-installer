interface LoadingIndicatorProps {
  label: string;
  screen?: boolean;
}

export function LoadingIndicator({ label, screen = false }: LoadingIndicatorProps) {
  if (screen) {
    return (
      <section className="loading-screen" role="status" aria-live="polite">
        <img src="/icon.png" alt="" />
        <div className="spinner" aria-hidden="true" />
        <p>{label}</p>
      </section>
    );
  }

  return (
    <div className="inline-loading" role="status" aria-live="polite">
      <div className="spinner" aria-hidden="true" />
      {label}
    </div>
  );
}
