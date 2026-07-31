import { api } from "../api";

const LINKS = [
  { label: "Aseprite", url: "https://www.aseprite.org/" },
  { label: "Aseprite on GitHub", url: "https://github.com/aseprite/aseprite" },
  {
    label: "Aseprite Installer on GitHub",
    url: "https://github.com/fmhun/asprite-installer",
  },
] as const;

export function AppFooter() {
  return (
    <footer className="app-footer" aria-label="Project links">
      {LINKS.map((link, index) => (
        <span className="footer-link-group" key={link.url}>
          {index > 0 && <span className="footer-separator" aria-hidden="true">·</span>}
          <button type="button" onClick={() => void api.openExternal(link.url)}>
            {link.label}
          </button>
        </span>
      ))}
    </footer>
  );
}
