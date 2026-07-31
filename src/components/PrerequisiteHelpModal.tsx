import { api } from "../api";
import { getPrerequisiteGuide } from "../prerequisiteHelp";
import type { Prerequisite } from "../types";
import { Modal } from "./Modal";

interface PrerequisiteHelpModalProps {
  prerequisite: Prerequisite;
  onClose: () => void;
}

export function PrerequisiteHelpModal({
  prerequisite,
  onClose,
}: PrerequisiteHelpModalProps) {
  const guide = getPrerequisiteGuide(prerequisite.id);

  return (
    <Modal
      ariaLabelledBy="manual-help-title"
      className="manual-help-modal"
      titlebar="MANUAL SETUP / macOS"
      onClose={onClose}
    >
      <div className="manual-help-content">
        <span className="manual-help-icon" aria-hidden="true">?</span>
        <h2 id="manual-help-title">{guide.title}</h2>
        <p className="manual-help-summary">{guide.summary}</p>
        {prerequisite.remediation && (
          <p className="manual-help-detected">
            <strong>Detected issue:</strong> {prerequisite.remediation}
          </p>
        )}
        <ol className="manual-help-steps">
          {guide.steps.map((step) => (
            <li key={step.title}>
              <h3>{step.title}</h3>
              <p>{step.body}</p>
              {step.command && <pre><code>{step.command}</code></pre>}
            </li>
          ))}
        </ol>
        <div className="manual-help-links">
          <strong>Official documentation</strong>
          {guide.links.map((link) => (
            <button
              className="text-link"
              type="button"
              key={link.url}
              onClick={() => void api.openExternal(link.url)}
            >
              {link.label} ↗
            </button>
          ))}
        </div>
      </div>
    </Modal>
  );
}
