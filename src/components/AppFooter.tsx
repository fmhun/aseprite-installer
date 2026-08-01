import { api } from "../api";

const LINKS = [
  { label: "Buy Aseprite", url: "https://www.aseprite.org/buy/" },
  { label: "Aseprite on GitHub", url: "https://github.com/aseprite/aseprite" },
  {
    label: "Aseprite Installer on GitHub",
    url: "https://github.com/fmhun/asprite-installer",
  },
] as const;

interface AppFooterProps {
  disclaimer: string;
}

export function AppFooter({ disclaimer }: AppFooterProps) {
  return (
    <footer className="app-footer" aria-label="Project links">
      <div className="footer-links">
        {LINKS.map((link, index) => (
          <span className="footer-link-group" key={link.url}>
            {index > 0 && <span className="footer-separator" aria-hidden="true">·</span>}
            <button type="button" onClick={() => void api.openExternal(link.url)}>
              {link.label}
            </button>
          </span>
        ))}
      </div>
      <p className="footer-disclaimer">{disclaimer}</p>
    </footer>
  );
}
