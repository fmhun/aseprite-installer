import { StrictMode } from "react";
import { createRoot, hydrateRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";

const container = document.getElementById("root")!;
const app = (
  <StrictMode>
    <App />
  </StrictMode>
);

if (container.dataset.prerendered === "true") {
  hydrateRoot(container, app);
} else {
  createRoot(container).render(app);
}
