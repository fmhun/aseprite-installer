import type { ReactNode } from "react";

interface ModalProps {
  ariaLabelledBy: string;
  children: ReactNode;
  className?: string;
  closeDisabled?: boolean;
  titlebar: string;
  onClose: () => void;
}

export function Modal({
  ariaLabelledBy,
  children,
  className = "",
  closeDisabled = false,
  titlebar,
  onClose,
}: ModalProps) {
  return (
    <div className="modal-backdrop" role="presentation">
      <section
        className={`modal ${className}`.trim()}
        role="dialog"
        aria-modal="true"
        aria-labelledby={ariaLabelledBy}
      >
        <header className="modal-titlebar">
          <span>{titlebar}</span>
          <button
            className="modal-close"
            type="button"
            aria-label="Close"
            disabled={closeDisabled}
            onClick={onClose}
          >
            ×
          </button>
        </header>
        <div className="modal-body">{children}</div>
      </section>
    </div>
  );
}

export function PixelDocumentIcon() {
  return (
    <svg
      className="pixel-document-icon"
      viewBox="0 0 16 18"
      shapeRendering="crispEdges"
      aria-hidden="true"
    >
      <path fill="currentColor" d="M2 1h8v2h2v2h2v12H2z" />
      <path fill="var(--workspace)" d="M4 3h5v4h3v8H4z" />
      <path fill="currentColor" d="M9 3h1v3h3v1H9zM5 9h6v1H5zM5 12h6v1H5z" />
    </svg>
  );
}
